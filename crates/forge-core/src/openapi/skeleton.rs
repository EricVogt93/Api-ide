//! Turning a parsed OpenAPI operation into a runnable [`RequestDef`]
//! skeleton (no assertions attached — see [`super::contract`] for that).

use serde_json::Value;

use crate::model::{BodyDef, KeyValue, Param, ParamKind, RequestDef};

use super::import::SpecOperation;

/// Build a request skeleton for one operation: URL (with `:param` path
/// placeholders), query/header/path parameter rows and a best-effort JSON
/// body derived from the request example or schema. No assertions attached.
pub fn operation_to_request(op: &SpecOperation) -> RequestDef {
    let url = format!("{{{{baseUrl}}}}{}", convert_path(&op.path));
    let name = if !op.summary.is_empty() {
        op.summary.clone()
    } else {
        op.id.clone()
    };
    let description = format!("{} {}", op.method.as_str(), op.path);

    let mut req = RequestDef::new(name, op.method, url);
    req.description = description;

    for name in &op.path_params {
        req.params.push(Param {
            kv: KeyValue::new(name.clone(), String::new()),
            kind: ParamKind::Path,
        });
    }
    for (name, required) in &op.query_params {
        let mut kv = KeyValue::new(name.clone(), String::new());
        kv.enabled = *required;
        req.params.push(Param {
            kv,
            kind: ParamKind::Query,
        });
    }
    for (name, _required) in &op.header_params {
        let mut kv = KeyValue::new(name.clone(), String::new());
        kv.enabled = false;
        req.headers.push(kv);
    }

    req.body = if let Some(example) = &op.request_example {
        BodyDef::Json {
            text: pretty_json(example),
        }
    } else if let Some(schema) = &op.request_schema {
        BodyDef::Json {
            text: pretty_json(&example_from_schema(schema, 0)),
        }
    } else {
        BodyDef::None
    };

    req
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Convert `/pets/{petId}` into `/pets/:petId`.
fn convert_path(path: &str) -> String {
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            out.push(':');
            for nc in chars.by_ref() {
                if nc == '}' {
                    break;
                }
                out.push(nc);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Generate a plausible example JSON value from a (already `$ref`-resolved)
/// JSON Schema. Prefers explicit `example`/`default`/`enum[0]` values, else
/// synthesizes one by type; objects are built required-property-first.
/// Recursion is capped at `depth` 6 to guard against runaway/cyclic schemas.
pub fn example_from_schema(schema: &Value, depth: u32) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    let Some(obj) = schema.as_object() else {
        return Value::Null;
    };

    if let Some(example) = obj.get("example") {
        return example.clone();
    }
    if let Some(default) = obj.get("default") {
        return default.clone();
    }
    if let Some(Value::Array(values)) = obj.get("enum") {
        if let Some(first) = values.first() {
            return first.clone();
        }
    }
    if let Some(Value::Array(variants)) = obj.get("oneOf").or_else(|| obj.get("anyOf")) {
        if let Some(first) = variants.first() {
            return example_from_schema(first, depth + 1);
        }
    }
    if let Some(Value::Array(parts)) = obj.get("allOf") {
        let mut merged = serde_json::Map::new();
        for part in parts {
            if let Value::Object(m) = example_from_schema(part, depth + 1) {
                merged.extend(m);
            }
        }
        if !merged.is_empty() {
            return Value::Object(merged);
        }
    }

    let schema_type = obj.get("type").and_then(schema_type_name);

    if schema_type.as_deref() == Some("object")
        || (schema_type.is_none() && obj.contains_key("properties"))
    {
        return build_object_example(obj, depth);
    }

    match schema_type.as_deref() {
        Some("string") => string_example(obj),
        Some("integer") => integer_example(obj),
        Some("number") => number_example(obj),
        Some("boolean") => Value::Bool(true),
        Some("array") => {
            let empty = Value::Object(serde_json::Map::new());
            let item_schema = obj.get("items").unwrap_or(&empty);
            let count = obj
                .get("minItems")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 32) as usize;
            Value::Array(vec![example_from_schema(item_schema, depth + 1); count])
        }
        Some("object") => Value::Object(serde_json::Map::new()),
        _ => Value::Null,
    }
}

/// Read a schema `type`, which may be a plain string (3.0) or an array
/// including `"null"` (3.1). Returns the first non-null type name.
fn schema_type_name(t: &Value) -> Option<String> {
    match t {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => arr
            .iter()
            .find_map(|v| v.as_str().filter(|s| *s != "null"))
            .map(str::to_string),
        _ => None,
    }
}

fn string_example(obj: &serde_json::Map<String, Value>) -> Value {
    let format = obj.get("format").and_then(Value::as_str);
    let mut value = match format {
        Some("email") => "user@example.com",
        Some("uuid") => "00000000-0000-0000-0000-000000000000",
        Some("date-time") => "2024-01-01T00:00:00Z",
        Some("date") => "2024-01-01",
        Some("uri") | Some("url") => "https://example.com",
        _ => "string",
    }
    .to_string();
    let min = obj
        .get("minLength")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .min(1024) as usize;
    while value.chars().count() < min {
        value.push('x');
    }
    if let Some(max) = obj.get("maxLength").and_then(Value::as_u64) {
        value = value.chars().take(max as usize).collect();
    }
    if let Some(pattern) = obj.get("pattern").and_then(Value::as_str) {
        if let Ok(regex) = regex::Regex::new(pattern) {
            if !regex.is_match(&value) {
                if let Some(candidate) = [
                    "ABC",
                    "abc",
                    "123",
                    "test",
                    "user@example.com",
                    "00000000-0000-0000-0000-000000000000",
                ]
                .into_iter()
                .find(|candidate| {
                    let len = candidate.chars().count();
                    len >= min
                        && obj
                            .get("maxLength")
                            .and_then(Value::as_u64)
                            .is_none_or(|max| len <= max as usize)
                        && regex.is_match(candidate)
                }) {
                    value = candidate.to_string();
                }
            }
        }
    }
    Value::String(value)
}

fn integer_example(obj: &serde_json::Map<String, Value>) -> Value {
    let minimum = obj
        .get("minimum")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let value = obj
        .get("exclusiveMinimum")
        .and_then(Value::as_i64)
        .map(|minimum| minimum.saturating_add(1))
        .unwrap_or(minimum);
    let value = obj
        .get("maximum")
        .and_then(Value::as_i64)
        .map_or(value, |maximum| value.min(maximum));
    Value::Number(value.into())
}

fn number_example(obj: &serde_json::Map<String, Value>) -> Value {
    let minimum = obj
        .get("minimum")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let value = obj
        .get("exclusiveMinimum")
        .and_then(Value::as_f64)
        .map(|minimum| minimum + f64::EPSILON)
        .unwrap_or(minimum);
    let value = obj
        .get("maximum")
        .and_then(Value::as_f64)
        .map_or(value, |maximum| value.min(maximum));
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn build_object_example(obj: &serde_json::Map<String, Value>, depth: u32) -> Value {
    let mut out = serde_json::Map::new();
    let Some(properties) = obj.get("properties").and_then(Value::as_object) else {
        return Value::Object(out);
    };
    if depth >= 6 {
        return Value::Object(out);
    }
    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut names: Vec<&String> = properties.keys().collect();
    names.sort_by_key(|n| !required.contains(&n.as_str()));

    for name in names {
        if let Some(pschema) = properties.get(name) {
            out.insert(name.clone(), example_from_schema(pschema, depth + 1));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Method;
    use crate::openapi::import::SpecResponse;

    fn op(path: &str) -> SpecOperation {
        SpecOperation {
            id: "getPetById".into(),
            method: Method::Get,
            path: path.into(),
            summary: "Get a pet".into(),
            tags: vec![],
            path_params: vec!["petId".into()],
            query_params: vec![("verbose".into(), false), ("limit".into(), true)],
            header_params: vec![("X-Trace-Id".into(), false)],
            request_content_type: None,
            request_schema: None,
            request_example: None,
            responses: vec![SpecResponse {
                status: "200".into(),
                content_type: None,
                schema: None,
            }],
        }
    }

    #[test]
    fn convert_path_uses_colon_params() {
        assert_eq!(
            convert_path("/pets/{petId}/tags/{tagId}"),
            "/pets/:petId/tags/:tagId"
        );
    }

    #[test]
    fn skeleton_url_and_params() {
        let o = op("/pets/{petId}");
        let req = operation_to_request(&o);
        assert_eq!(req.url, "{{baseUrl}}/pets/:petId");
        assert_eq!(req.name, "Get a pet");

        let path_param = req.params.iter().find(|p| p.kv.key == "petId").unwrap();
        assert_eq!(path_param.kind, ParamKind::Path);

        let required_q = req.params.iter().find(|p| p.kv.key == "limit").unwrap();
        assert!(required_q.kv.enabled);
        let optional_q = req.params.iter().find(|p| p.kv.key == "verbose").unwrap();
        assert!(!optional_q.kv.enabled);

        let header = req.headers.iter().find(|h| h.key == "X-Trace-Id").unwrap();
        assert!(!header.enabled);
    }

    #[test]
    fn skeleton_body_from_example() {
        let mut o = op("/pets/{petId}");
        o.request_example = Some(serde_json::json!({"name": "Rex"}));
        let req = operation_to_request(&o);
        match req.body {
            BodyDef::Json { text } => assert!(text.contains("Rex")),
            other => panic!("expected Json body, got {other:?}"),
        }
    }

    #[test]
    fn skeleton_body_from_schema() {
        let mut o = op("/pets/{petId}");
        o.request_schema = Some(serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            }
        }));
        let req = operation_to_request(&o);
        match req.body {
            BodyDef::Json { text } => {
                let v: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(v["name"], Value::String("string".into()));
                assert_eq!(v["age"], Value::Number(0.into()));
            }
            other => panic!("expected Json body, got {other:?}"),
        }
    }

    #[test]
    fn example_from_schema_nested_with_formats() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["id", "owner"],
            "properties": {
                "id": {"type": "string", "format": "uuid"},
                "createdAt": {"type": "string", "format": "date-time"},
                "owner": {
                    "type": "object",
                    "required": ["email"],
                    "properties": {
                        "email": {"type": "string", "format": "email"},
                        "nickname": {"type": "string"}
                    }
                },
                "tags": {"type": "array", "items": {"type": "string"}},
                "active": {"type": "boolean"}
            }
        });
        let example = example_from_schema(&schema, 0);
        assert_eq!(
            example["id"],
            Value::String("00000000-0000-0000-0000-000000000000".into())
        );
        assert_eq!(
            example["createdAt"],
            Value::String("2024-01-01T00:00:00Z".into())
        );
        assert_eq!(
            example["owner"]["email"],
            Value::String("user@example.com".into())
        );
        assert_eq!(example["tags"], serde_json::json!(["string"]));
        assert_eq!(example["active"], Value::Bool(true));
    }

    #[test]
    fn example_from_schema_prefers_explicit_example() {
        let schema = serde_json::json!({"type": "string", "example": "hello"});
        assert_eq!(
            example_from_schema(&schema, 0),
            Value::String("hello".into())
        );
    }

    #[test]
    fn example_from_schema_respects_basic_validation_limits() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["code", "count", "items"],
            "properties": {
                "code": {"type": "string", "pattern": "^[A-Z]{3}$", "minLength": 3},
                "count": {"type": "integer", "minimum": 4},
                "items": {"type": "array", "minItems": 2, "items": {"type": "boolean"}}
            }
        });

        let example = example_from_schema(&schema, 0);

        assert_eq!(example["code"], "ABC");
        assert_eq!(example["count"], 4);
        assert_eq!(example["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn example_from_schema_depth_capped() {
        // A schema that nests an object in itself indefinitely (via a cycle
        // simulated with repeated structure) must not blow the stack and
        // must stop producing new nesting past the cap.
        let mut schema = serde_json::json!({"type": "string"});
        for _ in 0..20 {
            schema = serde_json::json!({
                "type": "object",
                "properties": {"child": schema}
            });
        }
        let example = example_from_schema(&schema, 0);
        assert!(example.is_object());
    }
}
