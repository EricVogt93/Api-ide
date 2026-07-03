//! Evaluating [`Check`]s (and the [`AssertionDef`]s that wrap them) against
//! an [`ExecutionResult`].

use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;

use crate::exec::ExecutionResult;
use crate::model::{AssertionDef, Check, NumberOp, StringOp, ValueOp};

use super::outcome::AssertionOutcome;
use super::schema;

/// Evaluate every *enabled* assertion in `defs` against `res`, in order.
/// Disabled assertions are skipped entirely (no outcome is produced for
/// them).
pub fn evaluate_all(defs: &[AssertionDef], res: &ExecutionResult) -> Vec<AssertionOutcome> {
    defs.iter().filter(|d| d.enabled).map(|d| evaluate(&d.check, res)).collect()
}

/// Evaluate a single [`Check`] against `res`.
pub fn evaluate(check: &Check, res: &ExecutionResult) -> AssertionOutcome {
    let summary = check.summary();
    match check {
        Check::StatusCode { op, value } => eval_status_code(&summary, *op, *value, res),
        Check::StatusClass { class } => eval_status_class(&summary, *class, res),
        Check::Header { name, op, value } => eval_header(&summary, name, *op, value, res),
        Check::ContentType { value } => eval_content_type(&summary, value, res),
        Check::JsonPath { path, op, value } => eval_json_path(&summary, path, *op, value, res),
        Check::BodyContains { value } => eval_body_contains(&summary, value, res),
        Check::BodyMatches { regex } => eval_body_matches(&summary, regex, res),
        Check::ResponseTimeBelow { max_ms } => eval_response_time(&summary, *max_ms, res),
        Check::JsonSchema { schema } => eval_json_schema(&summary, schema, res),
    }
}

fn compare_numbers(actual: f64, op: NumberOp, expected: f64) -> bool {
    match op {
        NumberOp::Eq => actual == expected,
        NumberOp::Ne => actual != expected,
        NumberOp::Lt => actual < expected,
        NumberOp::Lte => actual <= expected,
        NumberOp::Gt => actual > expected,
        NumberOp::Gte => actual >= expected,
    }
}

fn eval_status_code(summary: &str, op: NumberOp, value: u16, res: &ExecutionResult) -> AssertionOutcome {
    let actual = res.status;
    if compare_numbers(actual as f64, op, value as f64) {
        AssertionOutcome::pass(summary)
    } else {
        AssertionOutcome::fail(
            summary,
            format!("expected status {} {value}, got {actual}", op.symbol()),
        )
    }
}

fn eval_status_class(summary: &str, class: u8, res: &ExecutionResult) -> AssertionOutcome {
    let actual_class = (res.status / 100) as u8;
    if actual_class == class {
        AssertionOutcome::pass(summary)
    } else {
        AssertionOutcome::fail(
            summary,
            format!("expected status class {class}xx, got {} ({actual_class}xx)", res.status),
        )
    }
}

fn eval_header(
    summary: &str,
    name: &str,
    op: StringOp,
    value: &str,
    res: &ExecutionResult,
) -> AssertionOutcome {
    let values = res.header_values(name);
    match op {
        StringOp::Exists => {
            if !values.is_empty() {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(summary, format!("expected header {name:?} to exist, but it was not present"))
            }
        }
        StringOp::NotExists => {
            if values.is_empty() {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(summary, format!("expected header {name:?} to not exist, got {values:?}"))
            }
        }
        StringOp::Equals => {
            if values.contains(&value) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected header {name:?} == {value:?}, got {values:?}"),
                )
            }
        }
        StringOp::NotEquals => {
            if !values.contains(&value) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected header {name:?} != {value:?}, got {values:?}"),
                )
            }
        }
        StringOp::Contains => {
            if values.iter().any(|v| v.contains(value)) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected header {name:?} to contain {value:?}, got {values:?}"),
                )
            }
        }
        StringOp::NotContains => {
            if !values.iter().any(|v| v.contains(value)) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected header {name:?} to not contain {value:?}, got {values:?}"),
                )
            }
        }
        StringOp::Matches => match Regex::new(value) {
            Ok(re) => {
                if values.iter().any(|v| re.is_match(v)) {
                    AssertionOutcome::pass(summary)
                } else {
                    AssertionOutcome::fail(
                        summary,
                        format!("expected header {name:?} to match /{value}/, got {values:?}"),
                    )
                }
            }
            Err(e) => AssertionOutcome::fail(summary, format!("invalid regex /{value}/: {e}")),
        },
    }
}

fn eval_content_type(summary: &str, value: &str, res: &ExecutionResult) -> AssertionOutcome {
    match res.content_type() {
        Some(ct) => {
            let mime = ct.split(';').next().unwrap_or("").trim();
            if mime.to_ascii_lowercase().starts_with(&value.to_ascii_lowercase()) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected content-type starting with {value:?}, got {mime:?}"),
                )
            }
        }
        None => AssertionOutcome::fail(
            summary,
            format!("expected content-type starting with {value:?}, got no content-type header"),
        ),
    }
}

fn eval_body_contains(summary: &str, value: &str, res: &ExecutionResult) -> AssertionOutcome {
    if res.text().contains(value) {
        AssertionOutcome::pass(summary)
    } else {
        AssertionOutcome::fail(summary, format!("expected body to contain {value:?}"))
    }
}

fn eval_body_matches(summary: &str, pattern: &str, res: &ExecutionResult) -> AssertionOutcome {
    match Regex::new(pattern) {
        Ok(re) => {
            if re.is_match(&res.text()) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(summary, format!("expected body to match /{pattern}/"))
            }
        }
        Err(e) => AssertionOutcome::fail(summary, format!("invalid regex /{pattern}/: {e}")),
    }
}

fn eval_response_time(summary: &str, max_ms: u64, res: &ExecutionResult) -> AssertionOutcome {
    let actual = res.timing.total.as_millis() as u64;
    if actual < max_ms {
        AssertionOutcome::pass(summary)
    } else {
        AssertionOutcome::fail(summary, format!("expected response time < {max_ms} ms, got {actual} ms"))
    }
}

fn eval_json_schema(summary: &str, schema_value: &Value, res: &ExecutionResult) -> AssertionOutcome {
    match res.json() {
        Some(instance) => match schema::validate(schema_value, &instance) {
            Ok(()) => AssertionOutcome::pass(summary),
            Err(errors) => AssertionOutcome::fail(summary, errors.join("; ")),
        },
        None => AssertionOutcome::fail(summary, "response body is not valid JSON"),
    }
}

fn value_op_symbol(op: ValueOp) -> &'static str {
    match op {
        ValueOp::Equals => "==",
        ValueOp::NotEquals => "!=",
        ValueOp::Contains => "contains",
        ValueOp::Matches => "matches",
        ValueOp::Exists => "exists",
        ValueOp::NotExists => "not exists",
        ValueOp::Lt => "<",
        ValueOp::Lte => "<=",
        ValueOp::Gt => ">",
        ValueOp::Gte => ">=",
    }
}

/// Number-aware JSON value equality: `1` and `1.0` compare equal.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Coerce a node to a string for substring/regex matching: strings are used
/// as-is (no surrounding quotes); everything else uses its compact JSON
/// representation.
fn node_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn node_contains(node: &Value, expected: &Value) -> bool {
    match node {
        Value::String(s) => s.contains(&node_to_string(expected)),
        Value::Array(arr) => arr.iter().any(|item| json_eq(item, expected)),
        Value::Object(map) => map.contains_key(&node_to_string(expected)),
        other => node_to_string(other).contains(&node_to_string(expected)),
    }
}

fn format_nodes(nodes: &[&Value]) -> String {
    nodes.iter().map(|n| node_to_string(n)).collect::<Vec<_>>().join(", ")
}

fn eval_json_path(
    summary: &str,
    path: &str,
    op: ValueOp,
    expected: &Value,
    res: &ExecutionResult,
) -> AssertionOutcome {
    let body = match res.json() {
        Some(v) => v,
        None => return AssertionOutcome::fail(summary, "response body is not valid JSON"),
    };
    let query = match JsonPath::parse(path) {
        Ok(q) => q,
        Err(e) => {
            return AssertionOutcome::fail(summary, format!("invalid JSONPath expression {path:?}: {e}"))
        }
    };
    let nodes: Vec<&Value> = query.query(&body).all();

    match op {
        ValueOp::Exists => {
            if !nodes.is_empty() {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(summary, format!("expected {path} to exist, but it had no match"))
            }
        }
        ValueOp::NotExists => {
            if nodes.is_empty() {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected {path} to not exist, got {} match(es)", nodes.len()),
                )
            }
        }
        ValueOp::Equals => {
            if nodes.is_empty() {
                AssertionOutcome::fail(summary, format!("expected {path} == {expected}, got no match"))
            } else if nodes.iter().any(|n| json_eq(n, expected)) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected {path} == {expected}, got {}", format_nodes(&nodes)),
                )
            }
        }
        ValueOp::NotEquals => {
            if nodes.is_empty() || !nodes.iter().any(|n| json_eq(n, expected)) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected {path} != {expected}, got {}", format_nodes(&nodes)),
                )
            }
        }
        ValueOp::Contains => {
            if nodes.is_empty() {
                AssertionOutcome::fail(summary, format!("expected {path} to contain {expected}, got no match"))
            } else if nodes.iter().any(|n| node_contains(n, expected)) {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!("expected {path} to contain {expected}, got {}", format_nodes(&nodes)),
                )
            }
        }
        ValueOp::Matches => {
            let pattern = expected.as_str().map(str::to_string).unwrap_or_else(|| expected.to_string());
            match Regex::new(&pattern) {
                Ok(re) => {
                    if nodes.is_empty() {
                        AssertionOutcome::fail(
                            summary,
                            format!("expected {path} to match /{pattern}/, got no match"),
                        )
                    } else if nodes.iter().any(|n| re.is_match(&node_to_string(n))) {
                        AssertionOutcome::pass(summary)
                    } else {
                        AssertionOutcome::fail(
                            summary,
                            format!("expected {path} to match /{pattern}/, got {}", format_nodes(&nodes)),
                        )
                    }
                }
                Err(e) => AssertionOutcome::fail(summary, format!("invalid regex /{pattern}/: {e}")),
            }
        }
        ValueOp::Lt | ValueOp::Lte | ValueOp::Gt | ValueOp::Gte => {
            let expected_num = match expected.as_f64() {
                Some(n) => n,
                None => {
                    return AssertionOutcome::fail(
                        summary,
                        format!("expected comparison value for {path} to be a number, got {expected}"),
                    )
                }
            };
            if nodes.is_empty() {
                return AssertionOutcome::fail(
                    summary,
                    format!("expected {path} to exist for a numeric comparison, got no match"),
                );
            }
            let numeric: Vec<f64> = nodes.iter().filter_map(|n| n.as_f64()).collect();
            if numeric.is_empty() {
                return AssertionOutcome::fail(
                    summary,
                    format!("expected {path} to be a number, got {}", format_nodes(&nodes)),
                );
            }
            let ok = numeric.iter().any(|&n| compare_numbers(n, to_number_op(op), expected_num));
            if ok {
                AssertionOutcome::pass(summary)
            } else {
                AssertionOutcome::fail(
                    summary,
                    format!(
                        "expected {path} {} {expected_num}, got {}",
                        value_op_symbol(op),
                        format_nodes(&nodes)
                    ),
                )
            }
        }
    }
}

/// Map the numeric-only subset of [`ValueOp`] onto [`NumberOp`] so we can
/// reuse `compare_numbers`. Only ever called for `Lt`/`Lte`/`Gt`/`Gte`.
fn to_number_op(op: ValueOp) -> NumberOp {
    match op {
        ValueOp::Lt => NumberOp::Lt,
        ValueOp::Lte => NumberOp::Lte,
        ValueOp::Gt => NumberOp::Gt,
        ValueOp::Gte => NumberOp::Gte,
        _ => unreachable!("to_number_op only called for numeric ValueOp variants"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::assert::test_support::exec_result;
    use crate::model::AssertionDef;

    fn json_res(status: u16, body: &Value, total_ms: u64) -> ExecutionResult {
        exec_result(
            status,
            &[("Content-Type", "application/json")],
            body.to_string().as_bytes(),
            total_ms,
        )
    }

    // ---- StatusCode / StatusClass ----

    #[test]
    fn status_code_eq_pass_and_fail() {
        let res = exec_result(200, &[], b"", 10);
        let pass = evaluate(&Check::StatusCode { op: NumberOp::Eq, value: 200 }, &res);
        assert!(pass.passed);

        let fail = evaluate(&Check::StatusCode { op: NumberOp::Eq, value: 201 }, &res);
        assert!(!fail.passed);
        assert_eq!(fail.message.as_deref(), Some("expected status == 201, got 200"));
    }

    #[test]
    fn status_code_gt() {
        let res = exec_result(404, &[], b"", 10);
        assert!(evaluate(&Check::StatusCode { op: NumberOp::Gt, value: 400 }, &res).passed);
        assert!(!evaluate(&Check::StatusCode { op: NumberOp::Gt, value: 500 }, &res).passed);
    }

    #[test]
    fn status_class() {
        let res = exec_result(404, &[], b"", 10);
        assert!(evaluate(&Check::StatusClass { class: 4 }, &res).passed);
        let fail = evaluate(&Check::StatusClass { class: 2 }, &res);
        assert!(!fail.passed);
        assert_eq!(fail.message.as_deref(), Some("expected status class 2xx, got 404 (4xx)"));
    }

    // ---- Header ----

    #[test]
    fn header_exists_not_exists() {
        let res = exec_result(200, &[("X-Foo", "bar")], b"", 10);
        assert!(evaluate(
            &Check::Header { name: "x-foo".into(), op: StringOp::Exists, value: String::new() },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::Header { name: "x-missing".into(), op: StringOp::Exists, value: String::new() },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::Header { name: "x-missing".into(), op: StringOp::NotExists, value: String::new() },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::Header { name: "x-foo".into(), op: StringOp::NotExists, value: String::new() },
            &res
        )
        .passed);
    }

    #[test]
    fn header_duplicate_values_equals_any() {
        let res = exec_result(200, &[("Set-Cookie", "a=1"), ("Set-Cookie", "b=2")], b"", 10);
        assert!(evaluate(
            &Check::Header { name: "set-cookie".into(), op: StringOp::Equals, value: "b=2".into() },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::Header { name: "set-cookie".into(), op: StringOp::Equals, value: "c=3".into() },
            &res
        )
        .passed);
    }

    #[test]
    fn header_contains_and_not_contains() {
        let res = exec_result(200, &[("X-Foo", "hello-world")], b"", 10);
        assert!(evaluate(
            &Check::Header { name: "X-Foo".into(), op: StringOp::Contains, value: "world".into() },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::Header { name: "X-Foo".into(), op: StringOp::NotContains, value: "nope".into() },
            &res
        )
        .passed);
    }

    #[test]
    fn header_matches_regex() {
        let res = exec_result(200, &[("X-Id", "req-42")], b"", 10);
        assert!(evaluate(
            &Check::Header { name: "x-id".into(), op: StringOp::Matches, value: r"^req-\d+$".into() },
            &res
        )
        .passed);
    }

    #[test]
    fn header_matches_invalid_regex_fails_gracefully() {
        let res = exec_result(200, &[("X-Id", "req-42")], b"", 10);
        let outcome = evaluate(
            &Check::Header { name: "x-id".into(), op: StringOp::Matches, value: "[".into() },
            &res,
        );
        assert!(!outcome.passed);
        assert!(outcome.message.unwrap().contains("invalid regex"));
    }

    // ---- ContentType ----

    #[test]
    fn content_type_ignores_params() {
        let res = exec_result(200, &[("Content-Type", "application/json; charset=utf-8")], b"{}", 10);
        assert!(evaluate(&Check::ContentType { value: "application/json".into() }, &res).passed);
    }

    #[test]
    fn content_type_missing_header_fails() {
        let res = exec_result(200, &[], b"", 10);
        let outcome = evaluate(&Check::ContentType { value: "application/json".into() }, &res);
        assert!(!outcome.passed);
        assert!(outcome.message.unwrap().contains("no content-type header"));
    }

    // ---- BodyContains / BodyMatches ----

    #[test]
    fn body_contains_and_matches() {
        let res = exec_result(200, &[], b"hello world", 10);
        assert!(evaluate(&Check::BodyContains { value: "world".into() }, &res).passed);
        assert!(!evaluate(&Check::BodyContains { value: "planet".into() }, &res).passed);
        assert!(evaluate(&Check::BodyMatches { regex: r"^hello \w+$".into() }, &res).passed);
        let bad = evaluate(&Check::BodyMatches { regex: "(".into() }, &res);
        assert!(!bad.passed);
        assert!(bad.message.unwrap().contains("invalid regex"));
    }

    // ---- ResponseTimeBelow ----

    #[test]
    fn response_time_below() {
        let res = exec_result(200, &[], b"", 100);
        assert!(evaluate(&Check::ResponseTimeBelow { max_ms: 200 }, &res).passed);
        assert!(!evaluate(&Check::ResponseTimeBelow { max_ms: 100 }, &res).passed);
        assert!(!evaluate(&Check::ResponseTimeBelow { max_ms: 50 }, &res).passed);
    }

    // ---- JsonPath ----

    #[test]
    fn json_path_non_json_body_fails() {
        let res = exec_result(200, &[], b"not json", 10);
        let outcome = evaluate(
            &Check::JsonPath { path: "$.a".into(), op: ValueOp::Exists, value: Value::Null },
            &res,
        );
        assert!(!outcome.passed);
        assert!(outcome.message.unwrap().contains("not valid JSON"));
    }

    #[test]
    fn json_path_invalid_expression_fails_gracefully() {
        let res = json_res(200, &json!({"a": 1}), 10);
        let outcome = evaluate(
            &Check::JsonPath { path: "not a path".into(), op: ValueOp::Exists, value: Value::Null },
            &res,
        );
        assert!(!outcome.passed);
        assert!(outcome.message.unwrap().contains("invalid JSONPath"));
    }

    #[test]
    fn json_path_exists_not_exists() {
        let res = json_res(200, &json!({"a": {"b": 1}}), 10);
        assert!(evaluate(
            &Check::JsonPath { path: "$.a.b".into(), op: ValueOp::Exists, value: Value::Null },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::JsonPath { path: "$.a.c".into(), op: ValueOp::NotExists, value: Value::Null },
            &res
        )
        .passed);
    }

    #[test]
    fn json_path_equals_numeric_coercion() {
        let res = json_res(200, &json!({"count": 1}), 10);
        assert!(evaluate(
            &Check::JsonPath { path: "$.count".into(), op: ValueOp::Equals, value: json!(1.0) },
            &res
        )
        .passed);
        let res2 = json_res(200, &json!({"count": 1.0}), 10);
        assert!(evaluate(
            &Check::JsonPath { path: "$.count".into(), op: ValueOp::Equals, value: json!(1) },
            &res2
        )
        .passed);
    }

    #[test]
    fn json_path_equals_multiple_nodes_any_match() {
        let res = json_res(200, &json!({"items": [{"v": 1}, {"v": 2}]}), 10);
        assert!(evaluate(
            &Check::JsonPath { path: "$.items[*].v".into(), op: ValueOp::Equals, value: json!(2) },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::JsonPath { path: "$.items[*].v".into(), op: ValueOp::Equals, value: json!(3) },
            &res
        )
        .passed);
    }

    #[test]
    fn json_path_contains_string_array_object() {
        let res = json_res(
            200,
            &json!({"s": "hello world", "arr": [1, 2, 3], "obj": {"k": 1}}),
            10,
        );
        assert!(evaluate(
            &Check::JsonPath { path: "$.s".into(), op: ValueOp::Contains, value: json!("world") },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::JsonPath { path: "$.arr".into(), op: ValueOp::Contains, value: json!(2) },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::JsonPath { path: "$.obj".into(), op: ValueOp::Contains, value: json!("k") },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::JsonPath { path: "$.obj".into(), op: ValueOp::Contains, value: json!("missing") },
            &res
        )
        .passed);
    }

    #[test]
    fn json_path_matches_regex() {
        let res = json_res(200, &json!({"id": "req-42"}), 10);
        assert!(evaluate(
            &Check::JsonPath {
                path: "$.id".into(),
                op: ValueOp::Matches,
                value: json!(r"^req-\d+$")
            },
            &res
        )
        .passed);
    }

    #[test]
    fn json_path_numeric_comparisons() {
        let res = json_res(200, &json!({"n": 5}), 10);
        assert!(evaluate(
            &Check::JsonPath { path: "$.n".into(), op: ValueOp::Gt, value: json!(1) },
            &res
        )
        .passed);
        assert!(evaluate(
            &Check::JsonPath { path: "$.n".into(), op: ValueOp::Lte, value: json!(5) },
            &res
        )
        .passed);
        assert!(!evaluate(
            &Check::JsonPath { path: "$.n".into(), op: ValueOp::Lt, value: json!(5) },
            &res
        )
        .passed);
    }

    #[test]
    fn json_path_numeric_comparison_non_number_fails() {
        let res = json_res(200, &json!({"n": "not-a-number"}), 10);
        let outcome = evaluate(
            &Check::JsonPath { path: "$.n".into(), op: ValueOp::Gt, value: json!(1) },
            &res,
        );
        assert!(!outcome.passed);
    }

    // ---- JsonSchema ----

    #[test]
    fn json_schema_pass_and_fail() {
        let schema = json!({"type": "object", "required": ["id"]});
        let res_ok = json_res(200, &json!({"id": 1}), 10);
        assert!(evaluate(&Check::JsonSchema { schema: schema.clone() }, &res_ok).passed);

        let res_bad = json_res(200, &json!({}), 10);
        let outcome = evaluate(&Check::JsonSchema { schema }, &res_bad);
        assert!(!outcome.passed);
    }

    // ---- evaluate_all / disabled skipping ----

    #[test]
    fn evaluate_all_skips_disabled() {
        let res = exec_result(200, &[], b"", 10);
        let defs = vec![
            AssertionDef { check: Check::StatusCode { op: NumberOp::Eq, value: 200 }, enabled: true, note: String::new() },
            AssertionDef { check: Check::StatusCode { op: NumberOp::Eq, value: 500 }, enabled: false, note: String::new() },
        ];
        let outcomes = evaluate_all(&defs, &res);
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].passed);
    }
}
