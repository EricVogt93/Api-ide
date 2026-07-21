//! Static metadata and input validation for shipped request-format-v1 assets.

use serde_json::Value;

use super::diag::{Code, Diagnostic};
use super::model::PipelinePhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinIntent {
    Validate,
    Prepare,
    Capture,
    Generate,
    Simulate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTarget {
    Binding,
    Pipeline(PipelinePhase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinParameterKind {
    String,
    Integer,
    Boolean,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinParameter {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: BuiltinParameterKind,
    pub required: bool,
    /// JSON text so string, number, boolean and structured defaults share one
    /// static representation.
    pub default: Option<&'static str>,
    pub options: &'static [&'static str],
    pub example: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinDefinition {
    pub name: &'static str,
    pub reference: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub intent: BuiltinIntent,
    pub target: BuiltinTarget,
    pub parameters: &'static [BuiltinParameter],
    /// Complete `with` object as JSON.
    pub example: &'static str,
}

/// Optional `<asset>.meta.json` data for user-owned executable assets.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAssetMetadata {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub intent: BuiltinIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PipelinePhase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ProjectAssetParameter>,
    #[serde(default)]
    pub example: Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAssetParameter {
    pub name: String,
    pub label: String,
    pub kind: BuiltinParameterKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub example: String,
}

use BuiltinIntent::{Capture, Generate, Prepare, Validate};
use BuiltinParameterKind::{Integer, Json, String as StringParam};
use BuiltinTarget::{Binding, Pipeline};
use PipelinePhase::{AfterResponse, BeforeRequest};

const EMPTY: &[BuiltinParameter] = &[];
const BEARER: &[BuiltinParameter] = &[
    param("token", "Token", StringParam, true, None, &[], "eyJ..."),
    param(
        "prefix",
        "Prefix",
        StringParam,
        false,
        Some("\"Bearer\""),
        &[],
        "Bearer",
    ),
];
const BASIC: &[BuiltinParameter] = &[
    param("username", "Username", StringParam, true, None, &[], "api"),
    param(
        "password",
        "Password",
        StringParam,
        true,
        None,
        &[],
        "secret",
    ),
];
const HEADER: &[BuiltinParameter] = &[
    param(
        "name",
        "Header name",
        StringParam,
        true,
        None,
        &[],
        "X-Trace-Id",
    ),
    param(
        "value",
        "Header value",
        StringParam,
        true,
        None,
        &[],
        "trace-123",
    ),
];
const STATUS: &[BuiltinParameter] = &[param(
    "expected",
    "Expected status",
    Integer,
    true,
    None,
    &[],
    "201",
)];
const JSON_PATH: &[BuiltinParameter] = &[
    param(
        "path",
        "JSONPath",
        StringParam,
        true,
        None,
        &[],
        "$.user.id",
    ),
    param(
        "operator",
        "Operator",
        StringParam,
        false,
        Some("\"exists\""),
        &["exists", "notExists", "equals", "contains"],
        "equals",
    ),
    param("value", "Expected value", Json, false, None, &[], "\"u-1\""),
];
const SCHEMA: &[BuiltinParameter] = &[param(
    "schema",
    "JSON Schema",
    Json,
    true,
    None,
    &[],
    r#"{"type":"object"}"#,
)];
const ASSERT_HEADER: &[BuiltinParameter] = &[
    param(
        "name",
        "Header name",
        StringParam,
        true,
        None,
        &[],
        "Content-Type",
    ),
    param(
        "value",
        "Expected value",
        StringParam,
        false,
        None,
        &[],
        "application/json",
    ),
];
const RESPONSE_TIME: &[BuiltinParameter] = &[param(
    "maxMs",
    "Maximum time (ms)",
    Integer,
    true,
    None,
    &[],
    "500",
)];
const BODY_TEXT: &[BuiltinParameter] = &[param(
    "text",
    "Text",
    StringParam,
    true,
    None,
    &[],
    "success",
)];
const BODY_REGEX: &[BuiltinParameter] = &[param(
    "pattern",
    "Regular expression",
    StringParam,
    true,
    None,
    &[],
    r#""id":\s*\d+"#,
)];
const JSON_PATH_TYPE: &[BuiltinParameter] = &[
    param("path", "JSONPath", StringParam, true, None, &[], "$.items"),
    param(
        "expected",
        "Expected type",
        StringParam,
        true,
        None,
        &["null", "boolean", "number", "string", "array", "object"],
        "array",
    ),
];
const JSON_PATH_LENGTH: &[BuiltinParameter] = &[
    param("path", "JSONPath", StringParam, true, None, &[], "$.items"),
    param(
        "operator",
        "Operator",
        StringParam,
        false,
        Some("\"equals\""),
        &["equals", "lt", "lte", "gt", "gte"],
        "equals",
    ),
    param("expected", "Expected length", Integer, true, None, &[], "3"),
];
const COOKIE: &[BuiltinParameter] = &[
    param(
        "name",
        "Cookie name",
        StringParam,
        true,
        None,
        &[],
        "session",
    ),
    param(
        "value",
        "Expected value",
        StringParam,
        false,
        None,
        &[],
        "abc123",
    ),
];
const OPENAPI_RESPONSE: &[BuiltinParameter] = &[
    param(
        "spec",
        "OpenAPI document",
        Json,
        false,
        None,
        &[],
        r#"{"openapi":"3.0.3","paths":{...}}"#,
    ),
    param(
        "specRef",
        "OpenAPI file reference",
        StringParam,
        false,
        None,
        &[],
        "../../specs/api.yaml",
    ),
    param(
        "method",
        "Method",
        StringParam,
        false,
        None,
        &[
            "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
        ],
        "GET",
    ),
    param(
        "url",
        "Request URL",
        StringParam,
        false,
        None,
        &[],
        "/pets/42",
    ),
];
const EXTRACT_JSON_PATH: &[BuiltinParameter] = &[
    param("path", "JSONPath", StringParam, true, None, &[], "$.token"),
    param(
        "target",
        "Runtime variable",
        StringParam,
        true,
        None,
        &[],
        "token",
    ),
];
const EXTRACT_HEADER: &[BuiltinParameter] = &[
    param(
        "name",
        "Header name",
        StringParam,
        true,
        None,
        &[],
        "X-Request-Id",
    ),
    param(
        "target",
        "Runtime variable",
        StringParam,
        true,
        None,
        &[],
        "requestId",
    ),
];

const fn param(
    name: &'static str,
    label: &'static str,
    kind: BuiltinParameterKind,
    required: bool,
    default: Option<&'static str>,
    options: &'static [&'static str],
    example: &'static str,
) -> BuiltinParameter {
    BuiltinParameter {
        name,
        label,
        kind,
        required,
        default,
        options,
        example,
    }
}

static BUILTINS: &[BuiltinDefinition] = &[
    builtin(
        "uuid",
        "builtin:uuid@1",
        "Generate UUID",
        "Generate a fresh UUID v4 binding.",
        Generate,
        Binding,
        EMPTY,
        "{}",
    ),
    builtin(
        "now",
        "builtin:now@1",
        "Generate timestamp",
        "Generate the current Unix timestamp.",
        Generate,
        Binding,
        EMPTY,
        "{}",
    ),
    builtin(
        "bearer",
        "builtin:bearer@1",
        "Bearer authentication",
        "Set the Authorization header with a bearer token.",
        Prepare,
        Pipeline(BeforeRequest),
        BEARER,
        r#"{"token":"${secret.apiToken}","prefix":"Bearer"}"#,
    ),
    builtin(
        "basic",
        "builtin:basic@1",
        "Basic authentication",
        "Set the Authorization header from username and password.",
        Prepare,
        Pipeline(BeforeRequest),
        BASIC,
        r#"{"username":"api","password":"${secret.password}"}"#,
    ),
    builtin(
        "header",
        "builtin:header@1",
        "Set request header",
        "Add or replace one request header.",
        Prepare,
        Pipeline(BeforeRequest),
        HEADER,
        r#"{"name":"X-Trace-Id","value":"${bindings.traceId}"}"#,
    ),
    builtin(
        "assert-status",
        "builtin:assert-status@1",
        "Status is",
        "Assert the exact HTTP response status.",
        Validate,
        Pipeline(AfterResponse),
        STATUS,
        r#"{"expected":201}"#,
    ),
    builtin(
        "assert-json-path",
        "builtin:assert-json-path@1",
        "JSONPath value",
        "Assert a JSONPath exists or matches a value.",
        Validate,
        Pipeline(AfterResponse),
        JSON_PATH,
        r#"{"path":"$.user.id","operator":"equals","value":"u-1"}"#,
    ),
    builtin(
        "assert-schema",
        "builtin:assert-schema@1",
        "JSON Schema",
        "Validate the JSON response body against a JSON Schema.",
        Validate,
        Pipeline(AfterResponse),
        SCHEMA,
        r#"{"schema":{"type":"object"}}"#,
    ),
    builtin(
        "assert-header",
        "builtin:assert-header@1",
        "Response header",
        "Assert a response header exists or equals a value.",
        Validate,
        Pipeline(AfterResponse),
        ASSERT_HEADER,
        r#"{"name":"Content-Type","value":"application/json"}"#,
    ),
    builtin(
        "assert-response-time",
        "builtin:assert-response-time@1",
        "Response time below",
        "Assert the response completed below a millisecond limit.",
        Validate,
        Pipeline(AfterResponse),
        RESPONSE_TIME,
        r#"{"maxMs":500}"#,
    ),
    builtin(
        "assert-body-text",
        "builtin:assert-body-text@1",
        "Body contains text",
        "Assert the response body contains text.",
        Validate,
        Pipeline(AfterResponse),
        BODY_TEXT,
        r#"{"text":"success"}"#,
    ),
    builtin(
        "assert-body-regex",
        "builtin:assert-body-regex@1",
        "Body matches regex",
        "Assert the response body matches a regular expression.",
        Validate,
        Pipeline(AfterResponse),
        BODY_REGEX,
        r#"{"pattern":"\"id\":\\s*\\d+"}"#,
    ),
    builtin(
        "assert-json-path-type",
        "builtin:assert-json-path-type@1",
        "JSONPath type",
        "Assert the JSON value at a path has the expected type.",
        Validate,
        Pipeline(AfterResponse),
        JSON_PATH_TYPE,
        r#"{"path":"$.items","expected":"array"}"#,
    ),
    builtin(
        "assert-json-path-length",
        "builtin:assert-json-path-length@1",
        "JSONPath length",
        "Assert the length of an array, object or string.",
        Validate,
        Pipeline(AfterResponse),
        JSON_PATH_LENGTH,
        r#"{"path":"$.items","operator":"gte","expected":1}"#,
    ),
    builtin(
        "assert-cookie",
        "builtin:assert-cookie@1",
        "Response cookie",
        "Assert a Set-Cookie response header contains a cookie.",
        Validate,
        Pipeline(AfterResponse),
        COOKIE,
        r#"{"name":"session","value":"abc123"}"#,
    ),
    builtin(
        "assert-openapi-response",
        "builtin:assert-openapi-response@1",
        "OpenAPI response",
        "Validate status, content type and body schema; provide either an inline OpenAPI document or a file reference.",
        Validate,
        Pipeline(AfterResponse),
        OPENAPI_RESPONSE,
        r#"{"spec":{"openapi":"3.0.3","info":{"title":"API","version":"1"},"paths":{}},"method":"GET","url":"/pets"}"#,
    ),
    builtin(
        "extract-json-path",
        "builtin:extract-json-path@1",
        "Capture JSONPath",
        "Store one JSONPath result as a runtime variable.",
        Capture,
        Pipeline(AfterResponse),
        EXTRACT_JSON_PATH,
        r#"{"path":"$.token","target":"token"}"#,
    ),
    builtin(
        "extract-header",
        "builtin:extract-header@1",
        "Capture response header",
        "Store one response header as a runtime variable.",
        Capture,
        Pipeline(AfterResponse),
        EXTRACT_HEADER,
        r#"{"name":"X-Request-Id","target":"requestId"}"#,
    ),
];

#[allow(clippy::too_many_arguments)]
const fn builtin(
    name: &'static str,
    reference: &'static str,
    title: &'static str,
    description: &'static str,
    intent: BuiltinIntent,
    target: BuiltinTarget,
    parameters: &'static [BuiltinParameter],
    example: &'static str,
) -> BuiltinDefinition {
    BuiltinDefinition {
        name,
        reference,
        title,
        description,
        intent,
        target,
        parameters,
        example,
    }
}

pub fn builtin_catalog() -> &'static [BuiltinDefinition] {
    BUILTINS
}

pub fn find_builtin(name: &str) -> Option<&'static BuiltinDefinition> {
    BUILTINS.iter().find(|definition| definition.name == name)
}

pub(crate) fn validate_builtin(
    name: &str,
    version: Option<u32>,
    target: BuiltinTarget,
    input: &Value,
    raw: &str,
) -> Result<&'static BuiltinDefinition, Diagnostic> {
    let definition = find_builtin(name).ok_or_else(|| {
        Diagnostic::new(
            Code::AssetNotFound,
            format!("unknown builtin asset {name:?}"),
        )
        .with_ref(raw)
    })?;
    if version.is_some_and(|version| version != 1) {
        return Err(Diagnostic::new(
            Code::UnsupportedAssetVersion,
            format!(
                "builtin {name}@{} not available (have @1)",
                version.unwrap()
            ),
        )
        .with_ref(raw));
    }
    if definition.target != target {
        return Err(Diagnostic::new(
            Code::IncompatibleAssetType,
            format!(
                "builtin {name:?} targets {:?}, not {:?}",
                definition.target, target
            ),
        )
        .with_ref(raw));
    }
    let object = input.as_object().ok_or_else(|| {
        Diagnostic::new(Code::InvalidAssetInput, "builtin input must be an object").with_ref(raw)
    })?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !definition.parameters.iter().any(|p| p.name == *key))
    {
        return Err(Diagnostic::new(
            Code::InvalidAssetInput,
            format!("unknown parameter {unknown:?} for builtin {name:?}"),
        )
        .with_ref(raw));
    }
    for parameter in definition.parameters {
        let Some(value) = object.get(parameter.name) else {
            if parameter.required {
                return Err(Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("missing required parameter {:?}", parameter.name),
                )
                .with_ref(raw));
            }
            continue;
        };
        let valid_type = match parameter.kind {
            BuiltinParameterKind::String => value.is_string(),
            BuiltinParameterKind::Integer => value.as_u64().is_some(),
            BuiltinParameterKind::Boolean => value.is_boolean(),
            BuiltinParameterKind::Json => true,
        };
        if !valid_type {
            return Err(Diagnostic::new(
                Code::InvalidAssetInput,
                format!(
                    "parameter {:?} must be {:?}",
                    parameter.name, parameter.kind
                ),
            )
            .with_ref(raw));
        }
        if parameter.required && value.as_str().is_some_and(str::is_empty) {
            return Err(Diagnostic::new(
                Code::InvalidAssetInput,
                format!("parameter {:?} must not be empty", parameter.name),
            )
            .with_ref(raw));
        }
        if !parameter.options.is_empty()
            && value
                .as_str()
                .is_none_or(|value| !parameter.options.contains(&value))
        {
            return Err(Diagnostic::new(
                Code::InvalidAssetInput,
                format!(
                    "parameter {:?} must be one of {}",
                    parameter.name,
                    parameter.options.join(", ")
                ),
            )
            .with_ref(raw));
        }
    }
    if let Some(path) = object.get("path").and_then(Value::as_str) {
        serde_json_path::JsonPath::parse(path).map_err(|error| {
            Diagnostic::new(
                Code::InvalidAssetInput,
                format!("invalid JSONPath {path:?}: {error}"),
            )
            .with_ref(raw)
        })?;
    }
    if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
        regex::Regex::new(pattern).map_err(|error| {
            Diagnostic::new(
                Code::InvalidAssetInput,
                format!("invalid regular expression {pattern:?}: {error}"),
            )
            .with_ref(raw)
        })?;
    }
    if name == "assert-json-path" {
        let operator = object
            .get("operator")
            .and_then(Value::as_str)
            .unwrap_or("exists");
        if matches!(operator, "equals" | "contains") && !object.contains_key("value") {
            return Err(Diagnostic::new(
                Code::InvalidAssetInput,
                format!("operator {operator:?} requires parameter \"value\""),
            )
            .with_ref(raw));
        }
        if operator == "contains" && object.get("value").is_some_and(|value| !value.is_string()) {
            return Err(Diagnostic::new(
                Code::InvalidAssetInput,
                "operator \"contains\" requires a string \"value\"",
            )
            .with_ref(raw));
        }
    }
    if name == "assert-response-time" && object.get("maxMs").and_then(Value::as_u64) == Some(0) {
        return Err(Diagnostic::new(
            Code::InvalidAssetInput,
            "parameter \"maxMs\" must be greater than zero",
        )
        .with_ref(raw));
    }
    if name == "assert-openapi-response" {
        match (object.contains_key("spec"), object.contains_key("specRef")) {
            (false, false) => {
                return Err(Diagnostic::new(
                    Code::InvalidAssetInput,
                    "OpenAPI response validation requires \"spec\" or \"specRef\"",
                )
                .with_ref(raw));
            }
            (true, true) => {
                return Err(Diagnostic::new(
                    Code::InvalidAssetInput,
                    "OpenAPI response validation accepts only one of \"spec\" and \"specRef\"",
                )
                .with_ref(raw));
            }
            _ => {}
        }
    }
    if name == "assert-status"
        && object
            .get("expected")
            .and_then(Value::as_u64)
            .is_some_and(|status| status > u16::MAX as u64)
    {
        return Err(Diagnostic::new(
            Code::InvalidAssetInput,
            "parameter \"expected\" exceeds the supported status range",
        )
        .with_ref(raw));
    }
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_is_unique_and_validates_inputs() {
        let mut names = std::collections::BTreeSet::new();
        assert!(builtin_catalog()
            .iter()
            .all(|definition| names.insert(definition.name)));
        assert!(find_builtin("assert-response-time").is_some());
        assert!(validate_builtin(
            "assert-status",
            Some(1),
            Pipeline(AfterResponse),
            &json!({"expected": 201}),
            "builtin:assert-status@1",
        )
        .is_ok());
        assert!(validate_builtin(
            "assert-status",
            Some(1),
            Pipeline(BeforeRequest),
            &json!({"expected": 201}),
            "builtin:assert-status@1",
        )
        .is_err());
        assert!(validate_builtin(
            "assert-status",
            Some(1),
            Pipeline(AfterResponse),
            &json!({}),
            "builtin:assert-status@1",
        )
        .is_err());
    }

    #[test]
    fn json_path_value_operators_require_compatible_values() {
        let validate = |input: Value| {
            validate_builtin(
                "assert-json-path",
                Some(1),
                Pipeline(AfterResponse),
                &input,
                "builtin:assert-json-path@1",
            )
        };

        for operator in ["equals", "contains"] {
            let error = validate(json!({"path": "$.id", "operator": operator})).unwrap_err();
            assert_eq!(error.code, Code::InvalidAssetInput.as_str());
            assert!(error.message.contains("requires parameter \"value\""));
        }
        let error = validate(json!({
            "path": "$.items",
            "operator": "contains",
            "value": 1
        }))
        .unwrap_err();
        assert!(error.message.contains("requires a string"));
        assert!(validate(json!({
            "path": "$.items",
            "operator": "contains",
            "value": "one"
        }))
        .is_ok());
        assert!(validate(json!({
            "path": "$.id",
            "operator": "equals",
            "value": null
        }))
        .is_ok());
    }
}
