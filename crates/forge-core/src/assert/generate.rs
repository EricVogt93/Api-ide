//! Generating a starter set of [`Check`]s from an observed response, so
//! users have something reasonable to edit rather than starting from a
//! blank test.

use serde_json::Value;

use crate::exec::ExecutionResult;
use crate::model::{Check, NumberOp, ValueOp};

/// Options controlling [`generate_from_response`].
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateOptions {
    /// How many levels deep into the JSON body to walk before falling back
    /// to a plain `Exists` check.
    pub max_depth: usize,
    /// Assert the actual observed value (`Equals`) instead of just its
    /// presence (`Exists`) for primitive leaves.
    pub include_values: bool,
    /// Hard cap on the number of checks generated (including the mandatory
    /// status/content-type/timing checks).
    pub max_assertions: usize,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self { max_depth: 2, include_values: true, max_assertions: 40 }
    }
}

/// Generate a starter set of assertions from a real response.
///
/// Always includes a `StatusCode` check for the observed status and a
/// `ResponseTimeBelow` check as the last entry. Adds a `ContentType` check
/// when a content-type header is present, and — for JSON bodies — walks the
/// body up to `opts.max_depth` generating one check per primitive field
/// (plus presence checks for arrays/objects), all subject to
/// `opts.max_assertions`.
pub fn generate_from_response(res: &ExecutionResult, opts: &GenerateOptions) -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(Check::StatusCode { op: NumberOp::Eq, value: res.status });

    if let Some(ct) = res.content_type() {
        let mime = ct.split(';').next().unwrap_or("").trim();
        if !mime.is_empty() {
            checks.push(Check::ContentType { value: mime.to_string() });
        }
    }

    if let Some(body) = res.json() {
        walk(&body, "$", 0, opts, &mut checks);
    }

    checks.push(Check::ResponseTimeBelow { max_ms: suggested_max_ms(res) });

    checks
}

/// `ceil(total_ms * 3, to the nearest 100ms)`, floored at 500ms — generous
/// enough to not be flaky on the same environment, tight enough to catch
/// real regressions.
fn suggested_max_ms(res: &ExecutionResult) -> u64 {
    let total_ms = res.timing.total.as_millis() as u64;
    let tripled = total_ms.saturating_mul(3);
    let rounded = tripled.saturating_add(99) / 100 * 100;
    rounded.max(500)
}

fn budget_exhausted(checks: &[Check], opts: &GenerateOptions) -> bool {
    // Reserve the last slot for the trailing `ResponseTimeBelow` check.
    checks.len() >= opts.max_assertions.saturating_sub(1)
}

fn walk(value: &Value, path: &str, depth: usize, opts: &GenerateOptions, checks: &mut Vec<Check>) {
    if budget_exhausted(checks, opts) {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.is_empty() || depth >= opts.max_depth {
                checks.push(exists_check(path));
                return;
            }
            for (key, val) in map {
                if budget_exhausted(checks, opts) {
                    break;
                }
                walk(val, &child_path(path, key), depth + 1, opts, checks);
            }
        }
        Value::Array(arr) => {
            checks.push(exists_check(path));
            if let Some(first) = arr.first() {
                if !budget_exhausted(checks, opts) {
                    let child = format!("{path}[0]");
                    if depth >= opts.max_depth {
                        checks.push(exists_check(&child));
                    } else {
                        walk(first, &child, depth + 1, opts, checks);
                    }
                }
            }
        }
        leaf => {
            if opts.include_values {
                checks.push(Check::JsonPath { path: path.to_string(), op: ValueOp::Equals, value: leaf.clone() });
            } else {
                checks.push(exists_check(path));
            }
        }
    }
}

fn exists_check(path: &str) -> Check {
    Check::JsonPath { path: path.to_string(), op: ValueOp::Exists, value: Value::Null }
}

/// A bare identifier can be written as `.key`; anything else (dots, spaces,
/// leading digits, …) needs RFC 9535 bracket-quote notation: `['key']`.
fn is_bare_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn child_path(parent: &str, key: &str) -> String {
    if is_bare_key(key) {
        format!("{parent}.{key}")
    } else {
        let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
        format!("{parent}['{escaped}']")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::assert::test_support::exec_result;

    fn res_with_json(status: u16, body: &Value, total_ms: u64) -> ExecutionResult {
        exec_result(
            status,
            &[("Content-Type", "application/json; charset=utf-8")],
            body.to_string().as_bytes(),
            total_ms,
        )
    }

    #[test]
    fn always_includes_status_and_time() {
        let res = exec_with_no_body(204, 10);
        let checks = generate_from_response(&res, &GenerateOptions::default());
        assert!(matches!(checks.first(), Some(Check::StatusCode { op: NumberOp::Eq, value: 204 })));
        assert!(matches!(checks.last(), Some(Check::ResponseTimeBelow { .. })));
    }

    fn exec_with_no_body(status: u16, total_ms: u64) -> ExecutionResult {
        exec_result(status, &[], b"", total_ms)
    }

    #[test]
    fn content_type_strips_params() {
        let res = res_with_json(200, &json!({"a": 1}), 10);
        let checks = generate_from_response(&res, &GenerateOptions::default());
        assert!(checks.iter().any(|c| matches!(
            c,
            Check::ContentType { value } if value == "application/json"
        )));
    }

    #[test]
    fn suggested_time_rounds_up_and_floors_at_500() {
        assert_eq!(suggested_max_ms(&exec_with_no_body(200, 0)), 500);
        assert_eq!(suggested_max_ms(&exec_with_no_body(200, 50)), 500); // 150 -> 200, floored to 500
        assert_eq!(suggested_max_ms(&exec_with_no_body(200, 200)), 600); // 600 -> 600
        assert_eq!(suggested_max_ms(&exec_with_no_body(200, 1000)), 3000);
    }

    #[test]
    fn bracket_notation_for_keys_with_dots_and_spaces() {
        let res = res_with_json(200, &json!({"weird.key": 1, "weird key": 2, "plain": 3}), 10);
        let checks = generate_from_response(&res, &GenerateOptions::default());
        let paths: Vec<&str> = checks
            .iter()
            .filter_map(|c| match c {
                Check::JsonPath { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert!(paths.contains(&"$['weird.key']"));
        assert!(paths.contains(&"$['weird key']"));
        assert!(paths.contains(&"$.plain"));
    }

    #[test]
    fn golden_nested_body() {
        let body = json!({
            "id": 42,
            "name": "widget",
            "active": true,
            "tags": ["a", "b"],
            "meta": {
                "created": "2024-01-01",
                "deep": { "x": 1 }
            },
            "empty_list": [],
            "empty_obj": {}
        });
        let res = res_with_json(200, &body, 42);
        let opts = GenerateOptions::default();
        let checks = generate_from_response(&res, &opts);

        let expected = vec![
            Check::StatusCode { op: NumberOp::Eq, value: 200 },
            Check::ContentType { value: "application/json".to_string() },
            Check::JsonPath { path: "$.id".into(), op: ValueOp::Equals, value: json!(42) },
            Check::JsonPath { path: "$.name".into(), op: ValueOp::Equals, value: json!("widget") },
            Check::JsonPath { path: "$.active".into(), op: ValueOp::Equals, value: json!(true) },
            Check::JsonPath { path: "$.tags".into(), op: ValueOp::Exists, value: Value::Null },
            Check::JsonPath { path: "$.tags[0]".into(), op: ValueOp::Equals, value: json!("a") },
            // meta is an object at depth 1 (< max_depth 2), so we descend into it;
            // its children are at depth 2 == max_depth, so they stop at Exists.
            Check::JsonPath { path: "$.meta.created".into(), op: ValueOp::Equals, value: json!("2024-01-01") },
            Check::JsonPath { path: "$.meta.deep".into(), op: ValueOp::Exists, value: Value::Null },
            Check::JsonPath { path: "$.empty_list".into(), op: ValueOp::Exists, value: Value::Null },
            Check::JsonPath { path: "$.empty_obj".into(), op: ValueOp::Exists, value: Value::Null },
            Check::ResponseTimeBelow { max_ms: suggested_max_ms(&res) },
        ];

        assert_eq!(checks, expected);
    }

    #[test]
    fn include_values_false_uses_exists_for_leaves() {
        let body = json!({"id": 42});
        let res = res_with_json(200, &body, 10);
        let opts = GenerateOptions { include_values: false, ..GenerateOptions::default() };
        let checks = generate_from_response(&res, &opts);
        assert!(checks.iter().any(|c| matches!(
            c,
            Check::JsonPath { path, op: ValueOp::Exists, .. } if path == "$.id"
        )));
    }

    #[test]
    fn max_assertions_is_respected() {
        let mut map = serde_json::Map::new();
        for i in 0..100 {
            map.insert(format!("field{i}"), json!(i));
        }
        let body = Value::Object(map);
        let res = res_with_json(200, &body, 10);
        let opts = GenerateOptions { max_assertions: 10, ..GenerateOptions::default() };
        let checks = generate_from_response(&res, &opts);
        assert!(checks.len() <= 10);
        // The trailing time check must still be present.
        assert!(matches!(checks.last(), Some(Check::ResponseTimeBelow { .. })));
    }

    #[test]
    fn non_json_body_only_generates_status_and_time() {
        let res = exec_result(200, &[("Content-Type", "text/plain")], b"hello", 10);
        let checks = generate_from_response(&res, &GenerateOptions::default());
        assert_eq!(checks.len(), 3); // status, content-type, time
        assert!(matches!(checks[0], Check::StatusCode { .. }));
        assert!(matches!(checks[1], Check::ContentType { .. }));
        assert!(matches!(checks[2], Check::ResponseTimeBelow { .. }));
    }
}
