//! Built-in pipeline assets and their execution against a response. All
//! builtins are Rust and satisfy the same contracts as project assets would
//! (§5, §9). Project (TS/JS) assets run on the QuickJS host and are a
//! documented extension point — not wired in v1.

use std::collections::BTreeMap;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;

use crate::model::{Check, Method};

use super::catalog::{validate_builtin, BuiltinTarget};
use super::diag::{Code, Diagnostic};
use super::ir::{ResolvedHeader, ResolvedPipelineEntry, ResolvedRequest};

/// A response, uniform across real HTTP and mocks, so assertions and
/// extractors run identically against either (§10).
#[derive(Debug, Clone)]
pub struct ResponseView {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub time_ms: u64,
}

impl ResponseView {
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One assertion outcome (§4).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    pub passed: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl AssertionResult {
    fn pass(message: impl Into<String>) -> Self {
        Self {
            passed: true,
            message: message.into(),
            expected: None,
            actual: None,
            path: None,
        }
    }
    fn fail(message: impl Into<String>, expected: Value, actual: Value) -> Self {
        Self {
            passed: false,
            message: message.into(),
            expected: Some(expected),
            actual: Some(actual),
            path: None,
        }
    }
}

/// A change a `beforeRequest` hook makes to the request (§4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestPatch {
    pub headers: Vec<ResolvedHeader>,
    pub url: Option<String>,
}

/// Output of running one `beforeRequest` builtin.
pub fn run_before_request(
    entry: &ResolvedPipelineEntry,
    _req: &ResolvedRequest,
) -> Result<RequestPatch, Diagnostic> {
    let name = entry.asset.address.as_str();
    let with = &entry.input;
    validate_builtin(
        name,
        entry.asset.version,
        BuiltinTarget::Pipeline(entry.phase),
        with,
        &entry.asset.raw,
    )?;
    let get_str = |k: &str| {
        with.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    match name {
        "bearer" => {
            let token = get_str("token");
            let prefix = with
                .get("prefix")
                .and_then(Value::as_str)
                .unwrap_or("Bearer");
            Ok(RequestPatch {
                headers: vec![ResolvedHeader {
                    name: "Authorization".to_string(),
                    value: format!("{prefix} {token}"),
                }],
                url: None,
            })
        }
        "basic" => {
            let token =
                BASE64_STANDARD.encode(format!("{}:{}", get_str("username"), get_str("password")));
            Ok(RequestPatch {
                headers: vec![ResolvedHeader {
                    name: "Authorization".to_string(),
                    value: format!("Basic {token}"),
                }],
                url: None,
            })
        }
        "header" => Ok(RequestPatch {
            headers: vec![ResolvedHeader {
                name: get_str("name"),
                value: get_str("value"),
            }],
            url: None,
        }),
        other => Err(hook_unknown(other, &entry.asset.raw)),
    }
}

/// Run one `afterResponse` builtin. Returns assertion results and/or runtime
/// updates; either may be empty depending on the asset kind.
pub fn run_after_response(
    entry: &ResolvedPipelineEntry,
    res: &ResponseView,
) -> Result<(Vec<AssertionResult>, BTreeMap<String, Value>), Diagnostic> {
    let name = entry.asset.address.as_str();
    let with = &entry.input;
    validate_builtin(
        name,
        entry.asset.version,
        BuiltinTarget::Pipeline(entry.phase),
        with,
        &entry.asset.raw,
    )?;

    match name {
        "assert-status" => {
            let expected = with.get("expected").and_then(Value::as_u64).unwrap_or(0) as u16;
            let r = if res.status == expected {
                AssertionResult::pass(format!("status is {expected}"))
            } else {
                AssertionResult::fail(
                    format!("status {} != {expected}", res.status),
                    expected.into(),
                    res.status.into(),
                )
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-json-path" => {
            let path = with.get("path").and_then(Value::as_str).unwrap_or_default();
            let op = with
                .get("operator")
                .and_then(Value::as_str)
                .unwrap_or("exists");
            let expected = with.get("value").cloned();
            Ok((
                vec![assert_json_path(res, path, op, expected)],
                BTreeMap::new(),
            ))
        }
        "assert-schema" => {
            let schema = with.get("schema").cloned().unwrap_or(Value::Null);
            let r = match res.json() {
                None => AssertionResult::fail(
                    "response body is not JSON".to_string(),
                    Value::Bool(true),
                    Value::Bool(false),
                ),
                Some(body) => match crate::assert::schema::validate(&schema, &body) {
                    Ok(()) => AssertionResult::pass("body matches JSON Schema"),
                    Err(errors) => AssertionResult::fail(
                        format!("body does not match schema: {}", errors.join("; ")),
                        Value::Null,
                        Value::Array(errors.into_iter().map(Value::from).collect()),
                    ),
                },
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-header" => {
            let hname = with.get("name").and_then(Value::as_str).unwrap_or_default();
            let expected = with.get("value").and_then(Value::as_str);
            let actual = res.header(hname);
            let r = match (expected, actual) {
                (Some(exp), Some(act)) if exp.eq_ignore_ascii_case(act) => {
                    AssertionResult::pass(format!("header {hname} == {exp}"))
                }
                (None, Some(_)) => AssertionResult::pass(format!("header {hname} present")),
                (_, actual) => AssertionResult::fail(
                    format!("header {hname} mismatch"),
                    expected.map(Value::from).unwrap_or(Value::Null),
                    actual.map(Value::from).unwrap_or(Value::Null),
                ),
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-response-time" => {
            let max_ms = with.get("maxMs").and_then(Value::as_u64).unwrap_or(0);
            let r = if res.time_ms < max_ms {
                AssertionResult::pass(format!("response time {} ms < {max_ms} ms", res.time_ms))
            } else {
                AssertionResult::fail(
                    format!("response time {} ms is not below {max_ms} ms", res.time_ms),
                    max_ms.into(),
                    res.time_ms.into(),
                )
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-body-text" => {
            let expected = with.get("text").and_then(Value::as_str).unwrap_or_default();
            let r = if res.text().contains(expected) {
                AssertionResult::pass(format!("body contains {expected:?}"))
            } else {
                AssertionResult::fail(
                    format!("body does not contain {expected:?}"),
                    expected.into(),
                    res.text().as_ref().into(),
                )
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-body-regex" => {
            let pattern = with
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let regex = Regex::new(pattern).map_err(|error| {
                Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("invalid regular expression {pattern:?}: {error}"),
                )
                .with_ref(&entry.asset.raw)
            })?;
            let r = if regex.is_match(&res.text()) {
                AssertionResult::pass(format!("body matches /{pattern}/"))
            } else {
                AssertionResult::fail(
                    format!("body does not match /{pattern}/"),
                    pattern.into(),
                    res.text().as_ref().into(),
                )
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-json-path-type" => {
            let path = with.get("path").and_then(Value::as_str).unwrap_or_default();
            let expected = with
                .get("expected")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let r = match query_one(res, path) {
                Ok(value) => {
                    let actual = json_type(&value);
                    if actual == expected {
                        AssertionResult::pass(format!("{path} has type {expected}"))
                    } else {
                        AssertionResult::fail(
                            format!("{path} has type {actual}, expected {expected}"),
                            expected.into(),
                            actual.into(),
                        )
                    }
                }
                Err(diagnostic) => {
                    AssertionResult::fail(diagnostic.message, expected.into(), Value::Null)
                }
            };
            Ok((vec![r.with_path(path)], BTreeMap::new()))
        }
        "assert-json-path-length" => {
            let path = with.get("path").and_then(Value::as_str).unwrap_or_default();
            let operator = with
                .get("operator")
                .and_then(Value::as_str)
                .unwrap_or("equals");
            let expected = with.get("expected").and_then(Value::as_u64).unwrap_or(0);
            let r = match query_one(res, path) {
                Ok(value) => match json_len(&value) {
                    Some(actual) => {
                        let passed = match operator {
                            "equals" => actual == expected,
                            "lt" => actual < expected,
                            "lte" => actual <= expected,
                            "gt" => actual > expected,
                            "gte" => actual >= expected,
                            _ => false,
                        };
                        if passed {
                            AssertionResult::pass(format!(
                                "{path} length {actual} {operator} {expected}"
                            ))
                        } else {
                            AssertionResult::fail(
                                format!("{path} length {actual} is not {operator} {expected}"),
                                expected.into(),
                                actual.into(),
                            )
                        }
                    }
                    None => AssertionResult::fail(
                        format!("{path} has no length"),
                        expected.into(),
                        json_type(&value).into(),
                    ),
                },
                Err(diagnostic) => {
                    AssertionResult::fail(diagnostic.message, expected.into(), Value::Null)
                }
            };
            Ok((vec![r.with_path(path)], BTreeMap::new()))
        }
        "assert-cookie" => {
            let name = with.get("name").and_then(Value::as_str).unwrap_or_default();
            let expected = with.get("value").and_then(Value::as_str);
            let actual = response_cookie(res, name);
            let r = match (expected, actual.as_deref()) {
                (None, Some(_)) => AssertionResult::pass(format!("cookie {name} is present")),
                (Some(expected), Some(actual)) if expected == actual => {
                    AssertionResult::pass(format!("cookie {name} equals expected"))
                }
                (_, actual) => AssertionResult::fail(
                    format!("cookie {name} mismatch"),
                    expected.map(Value::from).unwrap_or(Value::Null),
                    actual.map(Value::from).unwrap_or(Value::Null),
                ),
            };
            Ok((vec![r], BTreeMap::new()))
        }
        "assert-openapi-response" => {
            let assertions = assert_openapi_response(
                res,
                with.get("spec").unwrap_or(&Value::Null),
                with.get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                with.get("url").and_then(Value::as_str).unwrap_or_default(),
            )
            .map_err(|diagnostic| diagnostic.with_ref(&entry.asset.raw))?;
            Ok((assertions, BTreeMap::new()))
        }
        "extract-json-path" => {
            let path = with.get("path").and_then(Value::as_str).unwrap_or_default();
            let target = with
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut runtime = BTreeMap::new();
            match query_one(res, path) {
                Ok(v) => {
                    runtime.insert(target.to_string(), v);
                    Ok((Vec::new(), runtime))
                }
                Err(d) => Err(d.with_ref(&entry.asset.raw)),
            }
        }
        "extract-header" => {
            let name = with.get("name").and_then(Value::as_str).unwrap_or_default();
            let target = with
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let value = res.header(name).ok_or_else(|| {
                Diagnostic::new(
                    Code::AssetError,
                    format!("response header {name:?} is missing"),
                )
                .with_ref(&entry.asset.raw)
            })?;
            Ok((
                Vec::new(),
                BTreeMap::from([(target.to_string(), Value::String(value.to_string()))]),
            ))
        }
        other => Err(Diagnostic::new(
            Code::AssetNotFound,
            format!("unknown builtin afterResponse asset {other:?}"),
        )
        .with_ref(&entry.asset.raw)),
    }
}

fn assert_json_path(
    res: &ResponseView,
    path: &str,
    op: &str,
    expected: Option<Value>,
) -> AssertionResult {
    let found = query_one(res, path);
    let mut r = match (op, &found) {
        ("exists", Ok(_)) => AssertionResult::pass(format!("{path} exists")),
        ("exists", Err(_)) => AssertionResult::fail(
            format!("{path} does not exist"),
            Value::Bool(true),
            Value::Bool(false),
        ),
        ("notExists", Err(_)) => AssertionResult::pass(format!("{path} does not exist")),
        ("notExists", Ok(v)) => {
            AssertionResult::fail(format!("{path} exists"), Value::Null, v.clone())
        }
        ("equals", Ok(v)) => {
            let exp = expected.clone().unwrap_or(Value::Null);
            if *v == exp {
                AssertionResult::pass(format!("{path} equals expected"))
            } else {
                AssertionResult::fail(format!("{path} != expected"), exp, v.clone())
            }
        }
        ("contains", Ok(Value::String(s))) => {
            let needle = expected
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default();
            if s.contains(needle) {
                AssertionResult::pass(format!("{path} contains {needle:?}"))
            } else {
                AssertionResult::fail(
                    format!("{path} does not contain {needle:?}"),
                    expected.clone().unwrap_or(Value::Null),
                    Value::String(s.clone()),
                )
            }
        }
        (_, Err(d)) => AssertionResult::fail(d.message.clone(), Value::Null, Value::Null),
        (other, _) => AssertionResult::fail(
            format!("unsupported operator {other:?}"),
            Value::Null,
            Value::Null,
        ),
    };
    r.path = Some(path.to_string());
    r
}

impl AssertionResult {
    fn with_path(mut self, path: &str) -> Self {
        self.path = Some(path.to_string());
        self
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn json_len(value: &Value) -> Option<u64> {
    match value {
        Value::String(value) => Some(value.chars().count() as u64),
        Value::Array(value) => Some(value.len() as u64),
        Value::Object(value) => Some(value.len() as u64),
        _ => None,
    }
}

fn response_cookie(res: &ResponseView, name: &str) -> Option<String> {
    res.headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| cookie::Cookie::parse(value.clone()).ok())
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_string())
}

fn assert_openapi_response(
    res: &ResponseView,
    spec: &Value,
    method: &str,
    url: &str,
) -> Result<Vec<AssertionResult>, Diagnostic> {
    let source = match spec {
        Value::String(source) => source.clone(),
        other => serde_json::to_string(other).map_err(|error| {
            Diagnostic::new(
                Code::InvalidAssetInput,
                format!("cannot serialize OpenAPI document: {error}"),
            )
        })?,
    };
    let spec = crate::openapi::parse_spec(&source).map_err(|error| {
        Diagnostic::new(
            Code::InvalidAssetInput,
            format!("invalid OpenAPI document: {error}"),
        )
    })?;
    let method = Method::parse(method).ok_or_else(|| {
        Diagnostic::new(
            Code::InvalidAssetInput,
            format!("unsupported HTTP method {method:?}"),
        )
    })?;
    let operation = spec.find_operation(method, url).ok_or_else(|| {
        Diagnostic::new(
            Code::InvalidAssetInput,
            format!("OpenAPI operation not found for {method} {url}"),
        )
    })?;
    let checks = crate::openapi::contract_checks(operation, Some(res.status));
    if checks.is_empty() {
        return Ok(vec![AssertionResult::fail(
            format!("OpenAPI does not declare response status {}", res.status),
            Value::Array(
                operation
                    .responses
                    .iter()
                    .map(|response| Value::String(response.status.clone()))
                    .collect(),
            ),
            res.status.into(),
        )]);
    }

    Ok(checks
        .into_iter()
        .filter_map(|check| match check {
            Check::StatusCode { .. } | Check::StatusClass { .. } => Some(AssertionResult::pass(
                format!("OpenAPI declares response status {}", res.status),
            )),
            Check::ContentType { value: expected } => {
                let actual = res.header("content-type");
                let passed = actual.is_some_and(|actual| {
                    actual
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .eq_ignore_ascii_case(&expected)
                });
                Some(if passed {
                    AssertionResult::pass(format!("content-type matches OpenAPI ({expected})"))
                } else {
                    AssertionResult::fail(
                        "content-type does not match OpenAPI",
                        expected.into(),
                        actual.map(Value::from).unwrap_or(Value::Null),
                    )
                })
            }
            Check::JsonSchema { schema } => Some(match res.json() {
                None => AssertionResult::fail(
                    "response body is not JSON",
                    Value::Bool(true),
                    Value::Bool(false),
                ),
                Some(body) => match crate::assert::schema::validate(&schema, &body) {
                    Ok(()) => AssertionResult::pass("body matches OpenAPI response schema"),
                    Err(errors) => AssertionResult::fail(
                        format!(
                            "body does not match OpenAPI response schema: {}",
                            errors.join("; ")
                        ),
                        Value::Null,
                        Value::Array(errors.into_iter().map(Value::from).collect()),
                    ),
                },
            }),
            _ => None,
        })
        .collect())
}

/// Evaluate a JSONPath against the response body, requiring exactly one match.
pub(super) fn query_one(res: &ResponseView, path: &str) -> Result<Value, Diagnostic> {
    let body = res
        .json()
        .ok_or_else(|| Diagnostic::new(Code::AssetError, "response body is not JSON"))?;
    let query = JsonPath::parse(path).map_err(|e| {
        Diagnostic::new(Code::AssetError, format!("invalid JSONPath {path:?}: {e}"))
    })?;
    let nodes = query.query(&body).all();
    match nodes.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Diagnostic::new(
            Code::AssetError,
            format!("JSONPath {path:?} matched nothing"),
        )),
        many => Ok(Value::Array(many.iter().map(|v| (*v).clone()).collect())),
    }
}

fn hook_unknown(name: &str, raw: &str) -> Diagnostic {
    Diagnostic::new(
        Code::AssetNotFound,
        format!("unknown builtin beforeRequest asset {name:?}"),
    )
    .with_ref(raw)
}

/// True if this builtin ref is a known project-executable that v1 can't run —
/// used by the runner to emit a clear "not supported yet" diagnostic instead
/// of a generic unknown-asset error for `project:` refs.
pub fn is_project_asset(entry: &ResolvedPipelineEntry) -> bool {
    entry.asset.scheme != super::refs::RefScheme::Builtin
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(status: u16, body: &str) -> ResponseView {
        ResponseView {
            status,
            headers: vec![],
            body: body.as_bytes().to_vec(),
            time_ms: 1,
        }
    }
    fn entry(name: &str, with: Value) -> ResolvedPipelineEntry {
        ResolvedPipelineEntry {
            phase: super::super::model::PipelinePhase::AfterResponse,
            asset: super::super::refs::AssetDescriptor {
                raw: format!("builtin:{name}@1"),
                scheme: super::super::refs::RefScheme::Builtin,
                address: name.to_string(),
                pointer: None,
                version: Some(1),
            },
            input: with,
        }
    }

    #[test]
    fn assert_status_pass_fail() {
        let (r, _) = run_after_response(
            &entry("assert-status", json!({"expected":200})),
            &res(200, "{}"),
        )
        .unwrap();
        assert!(r[0].passed);
        let (r, _) = run_after_response(
            &entry("assert-status", json!({"expected":201})),
            &res(500, "{}"),
        )
        .unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn assert_json_path_equals_and_exists() {
        let body = r#"{"user":{"id":"u-1"}}"#;
        let (r, _) = run_after_response(
            &entry(
                "assert-json-path",
                json!({"path":"$.user.id","operator":"equals","value":"u-1"}),
            ),
            &res(200, body),
        )
        .unwrap();
        assert!(r[0].passed, "{:?}", r[0]);
        let (r, _) = run_after_response(
            &entry(
                "assert-json-path",
                json!({"path":"$.user.id","operator":"exists"}),
            ),
            &res(200, body),
        )
        .unwrap();
        assert!(r[0].passed);
        let (r, _) = run_after_response(
            &entry(
                "assert-json-path",
                json!({"path":"$.user.missing","operator":"exists"}),
            ),
            &res(200, body),
        )
        .unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn extract_json_path_writes_runtime() {
        let (a, rt) = run_after_response(
            &entry(
                "extract-json-path",
                json!({"path":"$.token","target":"tok"}),
            ),
            &res(200, r#"{"token":"abc"}"#),
        )
        .unwrap();
        assert!(a.is_empty());
        assert_eq!(rt.get("tok"), Some(&json!("abc")));
    }

    #[test]
    fn catalog_assertions_and_header_extractor_execute() {
        let response = ResponseView {
            status: 200,
            headers: vec![
                ("Set-Cookie".into(), "session=abc123; Path=/".into()),
                ("X-Request-Id".into(), "req-1".into()),
            ],
            body: br#"{"items":["alpha","beta"]}"#.to_vec(),
            time_ms: 42,
        };
        for (name, input) in [
            ("assert-response-time", json!({"maxMs": 100})),
            ("assert-body-text", json!({"text": "alpha"})),
            ("assert-body-regex", json!({"pattern": r#""items":\["#})),
            (
                "assert-json-path-type",
                json!({"path": "$.items", "expected": "array"}),
            ),
            (
                "assert-json-path-length",
                json!({"path": "$.items", "operator": "equals", "expected": 2}),
            ),
            (
                "assert-cookie",
                json!({"name": "session", "value": "abc123"}),
            ),
        ] {
            let (assertions, _) = run_after_response(&entry(name, input), &response).unwrap();
            assert!(assertions[0].passed, "{name}: {:?}", assertions[0]);
        }

        let (_, runtime) = run_after_response(
            &entry(
                "extract-header",
                json!({"name": "X-Request-Id", "target": "requestId"}),
            ),
            &response,
        )
        .unwrap();
        assert_eq!(runtime.get("requestId"), Some(&json!("req-1")));
    }

    #[test]
    fn openapi_response_uses_declared_content_and_schema() {
        let spec = json!({
            "openapi": "3.0.3",
            "info": {"title": "Test", "version": "1"},
            "paths": {
                "/pets/{id}": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["id"],
                                            "properties": {"id": {"type": "integer"}}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let input = json!({"spec": spec, "method": "GET", "url": "/pets/1"});
        let response = ResponseView {
            status: 200,
            headers: vec![(
                "Content-Type".into(),
                "application/json; charset=utf-8".into(),
            )],
            body: br#"{"id":1}"#.to_vec(),
            time_ms: 1,
        };
        let (assertions, _) =
            run_after_response(&entry("assert-openapi-response", input.clone()), &response)
                .unwrap();
        assert!(assertions.iter().all(|assertion| assertion.passed));

        let invalid = ResponseView {
            body: br#"{"id":"wrong"}"#.to_vec(),
            ..response
        };
        let (assertions, _) =
            run_after_response(&entry("assert-openapi-response", input), &invalid).unwrap();
        assert!(assertions.iter().any(|assertion| !assertion.passed));
    }

    #[test]
    fn bearer_hook_adds_authorization() {
        let e = ResolvedPipelineEntry {
            phase: super::super::model::PipelinePhase::BeforeRequest,
            asset: super::super::refs::AssetDescriptor {
                raw: "builtin:bearer@1".into(),
                scheme: super::super::refs::RefScheme::Builtin,
                address: "bearer".into(),
                pointer: None,
                version: Some(1),
            },
            input: json!({ "token": "t-1" }),
        };
        let req = ResolvedRequest {
            id: "x".into(),
            name: "x".into(),
            method: crate::model::Method::Get,
            url: "http://x".into(),
            headers: vec![],
            query: vec![],
            body: super::super::ir::ResolvedBody::None,
            pipeline: vec![],
            mock: None,
            bindings: json!({}),
            secret_values: vec![],
        };
        let patch = run_before_request(&e, &req).unwrap();
        assert_eq!(patch.headers[0].name, "Authorization");
        assert_eq!(patch.headers[0].value, "Bearer t-1");
    }
}
