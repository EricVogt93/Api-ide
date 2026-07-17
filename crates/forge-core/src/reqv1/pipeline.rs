//! Built-in pipeline assets and their execution against a response. All
//! builtins are Rust and satisfy the same contracts as project assets would
//! (§5, §9). Project (TS/JS) assets run on the QuickJS host and are a
//! documented extension point — not wired in v1.

use std::collections::BTreeMap;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde_json::Value;
use serde_json_path::JsonPath;

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
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
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
        Self { passed: true, message: message.into(), expected: None, actual: None, path: None }
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
#[derive(Debug, Default)]
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
    let get_str = |k: &str| with.get(k).and_then(Value::as_str).unwrap_or_default().to_string();

    match name {
        "bearer" => {
            let token = get_str("token");
            let prefix = with.get("prefix").and_then(Value::as_str).unwrap_or("Bearer");
            Ok(RequestPatch {
                headers: vec![ResolvedHeader {
                    name: "Authorization".to_string(),
                    value: format!("{prefix} {token}"),
                }],
                url: None,
            })
        }
        "basic" => {
            let token = BASE64_STANDARD.encode(format!("{}:{}", get_str("username"), get_str("password")));
            Ok(RequestPatch {
                headers: vec![ResolvedHeader {
                    name: "Authorization".to_string(),
                    value: format!("Basic {token}"),
                }],
                url: None,
            })
        }
        "header" => Ok(RequestPatch {
            headers: vec![ResolvedHeader { name: get_str("name"), value: get_str("value") }],
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
            let op = with.get("operator").and_then(Value::as_str).unwrap_or("exists");
            let expected = with.get("value").cloned();
            Ok((vec![assert_json_path(res, path, op, expected)], BTreeMap::new()))
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
        "extract-json-path" => {
            let path = with.get("path").and_then(Value::as_str).unwrap_or_default();
            let target = with.get("target").and_then(Value::as_str).unwrap_or_default();
            let mut runtime = BTreeMap::new();
            match query_one(res, path) {
                Ok(v) => {
                    runtime.insert(target.to_string(), v);
                    Ok((Vec::new(), runtime))
                }
                Err(d) => Err(d.with_ref(&entry.asset.raw)),
            }
        }
        other => Err(Diagnostic::new(
            Code::AssetNotFound,
            format!("unknown builtin afterResponse asset {other:?}"),
        )
        .with_ref(&entry.asset.raw)),
    }
}

fn assert_json_path(res: &ResponseView, path: &str, op: &str, expected: Option<Value>) -> AssertionResult {
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
            let needle = expected.as_ref().and_then(Value::as_str).unwrap_or_default();
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

/// Evaluate a JSONPath against the response body, requiring exactly one match.
fn query_one(res: &ResponseView, path: &str) -> Result<Value, Diagnostic> {
    let body = res
        .json()
        .ok_or_else(|| Diagnostic::new(Code::AssetError, "response body is not JSON"))?;
    let query = JsonPath::parse(path)
        .map_err(|e| Diagnostic::new(Code::AssetError, format!("invalid JSONPath {path:?}: {e}")))?;
    let nodes = query.query(&body).all();
    match nodes.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Diagnostic::new(Code::AssetError, format!("JSONPath {path:?} matched nothing"))),
        many => Ok(Value::Array(many.iter().map(|v| (*v).clone()).collect())),
    }
}

fn hook_unknown(name: &str, raw: &str) -> Diagnostic {
    Diagnostic::new(Code::AssetNotFound, format!("unknown builtin beforeRequest asset {name:?}"))
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
        ResponseView { status, headers: vec![], body: body.as_bytes().to_vec(), time_ms: 1 }
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
        let (r, _) = run_after_response(&entry("assert-status", json!({"expected":200})), &res(200, "{}")).unwrap();
        assert!(r[0].passed);
        let (r, _) = run_after_response(&entry("assert-status", json!({"expected":201})), &res(500, "{}")).unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn assert_json_path_equals_and_exists() {
        let body = r#"{"user":{"id":"u-1"}}"#;
        let (r, _) = run_after_response(
            &entry("assert-json-path", json!({"path":"$.user.id","operator":"equals","value":"u-1"})),
            &res(200, body),
        ).unwrap();
        assert!(r[0].passed, "{:?}", r[0]);
        let (r, _) = run_after_response(
            &entry("assert-json-path", json!({"path":"$.user.id","operator":"exists"})),
            &res(200, body),
        ).unwrap();
        assert!(r[0].passed);
        let (r, _) = run_after_response(
            &entry("assert-json-path", json!({"path":"$.user.missing","operator":"exists"})),
            &res(200, body),
        ).unwrap();
        assert!(!r[0].passed);
    }

    #[test]
    fn extract_json_path_writes_runtime() {
        let (a, rt) = run_after_response(
            &entry("extract-json-path", json!({"path":"$.token","target":"tok"})),
            &res(200, r#"{"token":"abc"}"#),
        ).unwrap();
        assert!(a.is_empty());
        assert_eq!(rt.get("tok"), Some(&json!("abc")));
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
            id: "x".into(), name: "x".into(), method: crate::model::Method::Get,
            url: "http://x".into(), headers: vec![], query: vec![],
            body: super::super::ir::ResolvedBody::None, pipeline: vec![], mock: None,
            bindings: json!({}), secret_values: vec![],
        };
        let patch = run_before_request(&e, &req).unwrap();
        assert_eq!(patch.headers[0].name, "Authorization");
        assert_eq!(patch.headers[0].value, "Bearer t-1");
    }
}
