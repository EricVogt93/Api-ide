//! Contract-test generation: turning an operation's declared responses into
//! [`Check`] assertions, and a whole spec into a ready-to-run request suite.

use serde_json::Value;

use crate::model::{AssertionDef, Check, NumberOp, RequestDef};

use super::import::{ParsedSpec, SpecOperation, SpecResponse};
use super::skeleton::operation_to_request;

/// Build the assertions that verify an operation's contract for a given
/// response `status` (exact HTTP status code), or — when `status` is
/// `None` — for the lowest declared `2xx` response.
///
/// Emits (in order, each only if applicable):
/// - `StatusCode` (exact) or `StatusClass` (for `NXX` pattern responses);
///   skipped for a `"default"` response since there's no fixed code to
///   assert against.
/// - `ContentType`, when the response declares one.
/// - `JsonSchema`, when the response declares a body schema (already
///   `$ref`-resolved and scrubbed of OpenAPI-only keywords).
///
/// Returns an empty list if no matching response is found.
pub fn contract_checks(op: &SpecOperation, status: Option<u16>) -> Vec<Check> {
    let resp = match status {
        Some(code) => find_response_for_status(op, code),
        None => lowest_2xx(op),
    };
    let Some(resp) = resp else { return Vec::new() };

    let mut checks = Vec::new();
    if let Some(code) = status {
        checks.push(Check::StatusCode {
            op: NumberOp::Eq,
            value: code,
        });
    } else if let Some(class) = status_class_of(&resp.status) {
        checks.push(Check::StatusClass { class });
    } else if let Ok(code) = resp.status.parse::<u16>() {
        checks.push(Check::StatusCode {
            op: NumberOp::Eq,
            value: code,
        });
    }
    // A "default" response has no fixed status to assert on.

    if let Some(ct) = &resp.content_type {
        checks.push(Check::ContentType { value: ct.clone() });
    }
    if let Some(schema) = &resp.schema {
        checks.push(Check::JsonSchema {
            schema: scrub_schema(schema.clone()),
        });
    }
    checks
}

fn find_response_for_status(op: &SpecOperation, code: u16) -> Option<&SpecResponse> {
    let exact = code.to_string();
    if let Some(r) = op.responses.iter().find(|r| r.status == exact) {
        return Some(r);
    }
    let class_pat = format!("{}XX", code / 100);
    if let Some(r) = op
        .responses
        .iter()
        .find(|r| r.status.eq_ignore_ascii_case(&class_pat))
    {
        return Some(r);
    }
    op.responses.iter().find(|r| r.status == "default")
}

fn lowest_2xx(op: &SpecOperation) -> Option<&SpecResponse> {
    op.responses
        .iter()
        .filter(|r| is_2xx(&r.status))
        .min_by_key(|r| status_sort_key(&r.status))
}

fn is_2xx(status: &str) -> bool {
    status.starts_with('2')
}

fn status_sort_key(status: &str) -> u32 {
    if let Ok(n) = status.parse::<u32>() {
        n
    } else if status.eq_ignore_ascii_case("2XX") {
        200
    } else {
        999
    }
}

fn status_class_of(status: &str) -> Option<u8> {
    let s = status.to_uppercase();
    let bytes = s.as_bytes();
    if bytes.len() == 3 && &bytes[1..] == b"XX" {
        (bytes[0] as char).to_digit(10).map(|d| d as u8)
    } else {
        None
    }
}

/// Strip OpenAPI-specific schema keywords that aren't valid JSON Schema
/// (or would trip up a strict validator), recursively:
/// - `nullable: true` becomes `type: [<type>, "null"]`.
/// - `discriminator`, `xml`, `example`, `externalDocs` are dropped.
pub fn scrub_schema(schema: Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut nullable = false;
            for (k, v) in map {
                match k.as_str() {
                    "nullable" => nullable = v.as_bool().unwrap_or(false),
                    "discriminator" | "xml" | "example" | "externalDocs" => {}
                    "properties" | "patternProperties" => {
                        if let Value::Object(inner) = v {
                            let scrubbed = inner
                                .into_iter()
                                .map(|(pk, pv)| (pk, scrub_schema(pv)))
                                .collect();
                            out.insert(k, Value::Object(scrubbed));
                        } else {
                            out.insert(k, v);
                        }
                    }
                    "items" | "not" | "additionalProperties" => {
                        out.insert(k, scrub_schema(v));
                    }
                    "allOf" | "oneOf" | "anyOf" => {
                        if let Value::Array(arr) = v {
                            out.insert(
                                k,
                                Value::Array(arr.into_iter().map(scrub_schema).collect()),
                            );
                        } else {
                            out.insert(k, v);
                        }
                    }
                    _ => {
                        out.insert(k, v);
                    }
                }
            }
            if nullable {
                match out.remove("type") {
                    Some(Value::String(s)) => {
                        out.insert(
                            "type".to_string(),
                            Value::Array(vec![Value::String(s), Value::String("null".into())]),
                        );
                        Value::Object(out)
                    }
                    Some(Value::Array(mut arr)) => {
                        if !arr.iter().any(|v| v.as_str() == Some("null")) {
                            arr.push(Value::String("null".into()));
                        }
                        out.insert("type".to_string(), Value::Array(arr));
                        Value::Object(out)
                    }
                    other_type => {
                        // No usable `type` to merge `null` into (e.g. a bare
                        // `allOf`/`$ref` composition) — injecting
                        // `type: "null"` here would make the schema only
                        // accept literal null. Wrap it as an `anyOf` instead
                        // so the original (non-null) shape still validates.
                        if let Some(t) = other_type {
                            out.insert("type".to_string(), t);
                        }
                        serde_json::json!({ "anyOf": [Value::Object(out), {"type": "null"}] })
                    }
                }
            } else {
                Value::Object(out)
            }
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(scrub_schema).collect()),
        other => other,
    }
}

/// Generate a full contract-test request suite: a skeleton request per
/// operation with `StatusCode`/`ContentType`/`JsonSchema` assertions
/// attached (against the lowest declared `2xx` response), each tagged with
/// the note `"contract"`. Paired with the operation id so callers can build
/// an [`crate::model::OpenApiBinding`].
pub fn contract_requests(spec: &ParsedSpec) -> Vec<(RequestDef, String)> {
    spec.operations
        .iter()
        .map(|op| {
            let mut req = operation_to_request(op);
            req.assertions = contract_checks(op, None)
                .into_iter()
                .map(|check| {
                    let mut def: AssertionDef = check.into();
                    def.note = "contract".to_string();
                    def
                })
                .collect();
            (req, op.id.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Method;

    fn op_with_responses(responses: Vec<SpecResponse>) -> SpecOperation {
        SpecOperation {
            id: "op".into(),
            method: Method::Get,
            path: "/x".into(),
            summary: "".into(),
            tags: vec![],
            path_params: vec![],
            query_params: vec![],
            header_params: vec![],
            request_content_type: None,
            request_schema: None,
            request_example: None,
            responses,
        }
    }

    #[test]
    fn contract_checks_emit_status_content_type_schema() {
        let schema =
            serde_json::json!({"type": "object", "properties": {"id": {"type": "integer"}}});
        let op = op_with_responses(vec![SpecResponse {
            status: "200".into(),
            content_type: Some("application/json".into()),
            schema: Some(schema),
        }]);
        let checks = contract_checks(&op, None);
        assert_eq!(checks.len(), 3);
        assert!(matches!(
            checks[0],
            Check::StatusCode {
                op: NumberOp::Eq,
                value: 200
            }
        ));
        assert!(matches!(&checks[1], Check::ContentType { value } if value == "application/json"));
        assert!(matches!(&checks[2], Check::JsonSchema { .. }));
    }

    #[test]
    fn contract_checks_uses_status_class_pattern() {
        let op = op_with_responses(vec![SpecResponse {
            status: "2XX".into(),
            content_type: None,
            schema: None,
        }]);
        let checks = contract_checks(&op, None);
        assert_eq!(checks, vec![Check::StatusClass { class: 2 }]);
    }

    #[test]
    fn contract_checks_none_when_no_2xx() {
        let op = op_with_responses(vec![SpecResponse {
            status: "404".into(),
            content_type: None,
            schema: None,
        }]);
        assert!(contract_checks(&op, None).is_empty());
    }

    #[test]
    fn contract_checks_explicit_status_skips_default_status_code() {
        let op = op_with_responses(vec![SpecResponse {
            status: "default".into(),
            content_type: Some("application/json".into()),
            schema: None,
        }]);
        let checks = contract_checks(&op, Some(500));
        // status explicitly requested -> StatusCode(500) still emitted since
        // the caller told us which code they're asserting against.
        assert!(matches!(checks[0], Check::StatusCode { value: 500, .. }));
    }

    #[test]
    fn scrub_schema_converts_nullable() {
        let schema = serde_json::json!({
            "type": "string",
            "nullable": true,
            "example": "should be dropped",
            "discriminator": {"propertyName": "kind"}
        });
        let scrubbed = scrub_schema(schema);
        assert_eq!(scrubbed["type"], serde_json::json!(["string", "null"]));
        assert!(scrubbed.get("example").is_none());
        assert!(scrubbed.get("discriminator").is_none());
        assert!(scrubbed.get("nullable").is_none());
    }

    #[test]
    fn scrub_schema_recurses_into_properties_and_items() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tag": {"type": "string", "nullable": true},
                "list": {"type": "array", "items": {"type": "integer", "nullable": true}}
            }
        });
        let scrubbed = scrub_schema(schema);
        assert_eq!(
            scrubbed["properties"]["tag"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            scrubbed["properties"]["list"]["items"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn scrub_schema_nullable_without_type_wraps_in_any_of() {
        // `allOf` + `nullable` with no sibling `type` — the previous
        // fallback injected `type: "null"`, which rejected every
        // non-null (but otherwise conforming) body.
        let schema = serde_json::json!({
            "allOf": [
                {"type": "object", "required": ["id"], "properties": {"id": {"type": "integer"}}}
            ],
            "nullable": true
        });
        let scrubbed = scrub_schema(schema);
        assert!(scrubbed.get("nullable").is_none());

        let good = serde_json::json!({"id": 1});
        assert!(
            jsonschema::is_valid(&scrubbed, &good),
            "expected non-null conforming body to validate"
        );
        assert!(
            jsonschema::is_valid(&scrubbed, &Value::Null),
            "expected null to still validate"
        );

        let bad = serde_json::json!({"id": "not-a-number"});
        assert!(!jsonschema::is_valid(&scrubbed, &bad));
    }

    #[test]
    fn json_schema_check_validates_conforming_and_rejects_violating_body() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["id", "name"],
            "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"}
            }
        });
        let op = op_with_responses(vec![SpecResponse {
            status: "200".into(),
            content_type: Some("application/json".into()),
            schema: Some(schema),
        }]);
        let checks = contract_checks(&op, None);
        let Check::JsonSchema { schema } = checks.into_iter().nth(2).unwrap() else {
            panic!("expected JsonSchema check");
        };

        let good = serde_json::json!({"id": 1, "name": "Rex"});
        assert!(jsonschema::is_valid(&schema, &good));

        let bad = serde_json::json!({"id": "not-a-number"});
        assert!(!jsonschema::is_valid(&schema, &bad));
    }

    #[test]
    fn contract_requests_attaches_assertions_with_contract_note() {
        let schema = serde_json::json!({"type": "object"});
        let op = SpecOperation {
            id: "getX".into(),
            method: Method::Get,
            path: "/x".into(),
            summary: "Get X".into(),
            tags: vec![],
            path_params: vec![],
            query_params: vec![],
            header_params: vec![],
            request_content_type: None,
            request_schema: None,
            request_example: None,
            responses: vec![SpecResponse {
                status: "200".into(),
                content_type: Some("application/json".into()),
                schema: Some(schema),
            }],
        };
        let spec = ParsedSpec {
            title: "T".into(),
            version: "1".into(),
            servers: vec!["/".into()],
            operations: vec![op],
            raw: Value::Null,
        };
        let generated = contract_requests(&spec);
        assert_eq!(generated.len(), 1);
        let (req, op_id) = &generated[0];
        assert_eq!(op_id, "getX");
        assert!(!req.assertions.is_empty());
        assert!(req.assertions.iter().all(|a| a.note == "contract"));
    }
}
