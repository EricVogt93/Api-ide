//! Extracting values out of a response into named variables, per
//! [`Extractor`] definitions attached to a request.

use regex::Regex;
use serde_json::Value;
use serde_json_path::JsonPath;

use crate::exec::ExecutionResult;
use crate::model::{Extractor, ExtractorSource};

/// Outcome of running a set of [`Extractor`]s against a response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractReport {
    /// Successfully extracted `(var, value)` pairs, in extractor order.
    pub values: Vec<(String, String)>,
    /// One human-readable message per extractor that could not be resolved.
    pub errors: Vec<String>,
}

/// Run every *enabled* extractor against `res`, collecting successes and
/// failures separately. Disabled extractors are skipped entirely.
pub fn apply_extractors(extractors: &[Extractor], res: &ExecutionResult) -> ExtractReport {
    let mut report = ExtractReport::default();
    for ext in extractors {
        if !ext.enabled {
            continue;
        }
        match extract_one(ext, res) {
            Ok(value) => report.values.push((ext.var.clone(), value)),
            Err(message) => report.errors.push(format!("{}: {message}", ext.var)),
        }
    }
    report
}

fn extract_one(ext: &Extractor, res: &ExecutionResult) -> Result<String, String> {
    match &ext.source {
        ExtractorSource::JsonPath { expr } => {
            let body = res.json().ok_or_else(|| "response body is not valid JSON".to_string())?;
            let query =
                JsonPath::parse(expr).map_err(|e| format!("invalid JSONPath expression {expr:?}: {e}"))?;
            let node = query.query(&body).first().ok_or_else(|| format!("no match for {expr}"))?;
            Ok(stringify_node(node))
        }
        ExtractorSource::Header { name } => {
            res.header(name).map(str::to_string).ok_or_else(|| format!("header {name:?} not found"))
        }
        ExtractorSource::Regex { pattern, group } => {
            let re = Regex::new(pattern).map_err(|e| format!("invalid regex /{pattern}/: {e}"))?;
            let text = res.text();
            let caps = re.captures(&text).ok_or_else(|| format!("pattern /{pattern}/ did not match"))?;
            caps.get(*group)
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| format!("capture group {group} not found"))
        }
    }
}

/// Stringify a JSONPath match: strings are used as-is (no surrounding
/// quotes), everything else uses its compact JSON representation.
fn stringify_node(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::assert::test_support::exec_result;
    use crate::model::ExtractScope;

    fn extractor(source: ExtractorSource, var: &str) -> Extractor {
        Extractor { source, var: var.to_string(), scope: ExtractScope::Runtime, enabled: true }
    }

    #[test]
    fn json_path_extraction_strips_string_quotes() {
        let res = exec_result(
            200,
            &[("Content-Type", "application/json")],
            json!({"token": "abc123", "count": 5, "nested": {"id": 7}}).to_string().as_bytes(),
            10,
        );
        let extractors = vec![
            extractor(ExtractorSource::JsonPath { expr: "$.token".into() }, "token"),
            extractor(ExtractorSource::JsonPath { expr: "$.count".into() }, "count"),
            extractor(ExtractorSource::JsonPath { expr: "$.nested".into() }, "nested"),
        ];
        let report = apply_extractors(&extractors, &res);
        assert!(report.errors.is_empty());
        assert_eq!(
            report.values,
            vec![
                ("token".to_string(), "abc123".to_string()),
                ("count".to_string(), "5".to_string()),
                ("nested".to_string(), "{\"id\":7}".to_string()),
            ]
        );
    }

    #[test]
    fn json_path_extraction_errors() {
        let res = exec_result(200, &[], b"not json", 10);
        let extractors =
            vec![extractor(ExtractorSource::JsonPath { expr: "$.a".into() }, "a")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("a: response body is not valid JSON"));
    }

    #[test]
    fn json_path_extraction_no_match() {
        let res = exec_result(200, &[], b"{\"a\":1}", 10);
        let extractors = vec![extractor(ExtractorSource::JsonPath { expr: "$.b".into() }, "b")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("no match"));
    }

    #[test]
    fn header_extraction_case_insensitive_first() {
        let res = exec_result(200, &[("X-Request-Id", "req-1"), ("X-Request-Id", "req-2")], b"", 10);
        let extractors =
            vec![extractor(ExtractorSource::Header { name: "x-request-id".into() }, "rid")];
        let report = apply_extractors(&extractors, &res);
        assert_eq!(report.values, vec![("rid".to_string(), "req-1".to_string())]);
    }

    #[test]
    fn header_extraction_missing() {
        let res = exec_result(200, &[], b"", 10);
        let extractors = vec![extractor(ExtractorSource::Header { name: "x-missing".into() }, "m")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert!(report.errors[0].contains("not found"));
    }

    #[test]
    fn regex_extraction_capture_group() {
        let res = exec_result(200, &[], b"session=abc-123-xyz;", 10);
        let extractors = vec![extractor(
            ExtractorSource::Regex { pattern: r"session=([a-z0-9-]+);".into(), group: 1 },
            "session",
        )];
        let report = apply_extractors(&extractors, &res);
        assert_eq!(report.values, vec![("session".to_string(), "abc-123-xyz".to_string())]);
    }

    #[test]
    fn regex_extraction_whole_match_group_zero() {
        let res = exec_result(200, &[], b"foo123bar", 10);
        let extractors =
            vec![extractor(ExtractorSource::Regex { pattern: r"\d+".into(), group: 0 }, "num")];
        let report = apply_extractors(&extractors, &res);
        assert_eq!(report.values, vec![("num".to_string(), "123".to_string())]);
    }

    #[test]
    fn regex_extraction_invalid_pattern_errors() {
        let res = exec_result(200, &[], b"foo", 10);
        let extractors = vec![extractor(ExtractorSource::Regex { pattern: "(".into(), group: 0 }, "x")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert!(report.errors[0].contains("invalid regex"));
    }

    #[test]
    fn regex_extraction_no_match_errors() {
        let res = exec_result(200, &[], b"foo", 10);
        let extractors = vec![extractor(ExtractorSource::Regex { pattern: "bar".into(), group: 0 }, "x")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert!(report.errors[0].contains("did not match"));
    }

    #[test]
    fn regex_extraction_missing_group_errors() {
        let res = exec_result(200, &[], b"foo123", 10);
        let extractors =
            vec![extractor(ExtractorSource::Regex { pattern: r"foo(\d+)?".into(), group: 5 }, "x")];
        let report = apply_extractors(&extractors, &res);
        assert!(report.values.is_empty());
        assert!(report.errors[0].contains("capture group 5 not found"));
    }

    #[test]
    fn disabled_extractors_are_skipped() {
        let res = exec_result(200, &[], b"{\"a\":1}", 10);
        let mut ext = extractor(ExtractorSource::JsonPath { expr: "$.a".into() }, "a");
        ext.enabled = false;
        let report = apply_extractors(&[ext], &res);
        assert!(report.values.is_empty());
        assert!(report.errors.is_empty());
    }
}
