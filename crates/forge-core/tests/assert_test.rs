//! Integration tests for the `assert` module: evaluation, generation and
//! extraction working together against a realistic response fixture.

use std::time::Duration;

use chrono::Utc;
use forge_core::assert::{
    apply_extractors, evaluate, evaluate_all, generate_from_response, GenerateOptions,
};
use forge_core::exec::{ExecutionResult, Sizes, TimingBreakdown};
use forge_core::model::{
    AssertionDef, Check, ExtractScope, Extractor, ExtractorSource, NumberOp, StringOp, ValueOp,
};

fn load_fixture_response(status: u16, total_ms: u64) -> ExecutionResult {
    let body = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/assert/nested_response.json"
    ))
    .expect("fixture should exist");

    ExecutionResult {
        status,
        status_text: "OK".to_string(),
        http_version: "HTTP/1.1".to_string(),
        headers: vec![
            (
                "Content-Type".to_string(),
                "application/json; charset=utf-8".to_string(),
            ),
            ("X-Request-Id".to_string(), "req-77".to_string()),
        ],
        body,
        timing: TimingBreakdown {
            total: Duration::from_millis(total_ms),
            ..Default::default()
        },
        size: Sizes::default(),
        effective_url: "https://api.example.test/items/7".to_string(),
        redirect_chain: Vec::new(),
        cookies_set: Vec::new(),
        executed_at: Utc::now(),
    }
}

#[test]
fn generate_then_evaluate_all_pass_on_the_same_response() {
    let res = load_fixture_response(200, 30);
    let opts = GenerateOptions::default();
    let generated = generate_from_response(&res, &opts);

    // Sanity: mandatory checks present.
    assert!(matches!(
        generated.first(),
        Some(Check::StatusCode {
            op: NumberOp::Eq,
            value: 200
        })
    ));
    assert!(matches!(
        generated.last(),
        Some(Check::ResponseTimeBelow { .. })
    ));
    assert!(generated
        .iter()
        .any(|c| matches!(c, Check::ContentType { value } if value == "application/json")));

    // A generated assertion set evaluated against the very response it was
    // generated from must pass in full.
    let defs: Vec<AssertionDef> = generated.into_iter().map(AssertionDef::from).collect();
    let outcomes = evaluate_all(&defs, &res);
    assert_eq!(outcomes.len(), defs.len());
    for outcome in &outcomes {
        assert!(
            outcome.passed,
            "expected {} to pass, got {:?}",
            outcome.summary, outcome.message
        );
    }
}

#[test]
fn generation_uses_bracket_notation_for_dotted_key() {
    let res = load_fixture_response(200, 10);
    let generated = generate_from_response(&res, &GenerateOptions::default());
    let has_bracket_path = generated.iter().any(|c| match c {
        Check::JsonPath { path, .. } => path == "$['weird.field']",
        _ => false,
    });
    assert!(
        has_bracket_path,
        "expected a bracket-quoted path for the dotted key"
    );
}

#[test]
fn manual_assertions_against_fixture() {
    let res = load_fixture_response(404, 10);

    let checks = vec![
        Check::StatusCode {
            op: NumberOp::Eq,
            value: 404,
        },
        Check::StatusClass { class: 4 },
        Check::Header {
            name: "x-request-id".into(),
            op: StringOp::Matches,
            value: r"^req-\d+$".into(),
        },
        Check::JsonPath {
            path: "$.pricing.amount".into(),
            op: ValueOp::Gt,
            value: serde_json::json!(10),
        },
        Check::JsonPath {
            path: "$.tags".into(),
            op: ValueOp::Contains,
            value: serde_json::json!("featured"),
        },
    ];

    for check in &checks {
        let outcome = evaluate(check, &res);
        assert!(
            outcome.passed,
            "{} failed: {:?}",
            outcome.summary, outcome.message
        );
    }

    // And one that should fail, with a precise message.
    let failing = Check::StatusCode {
        op: NumberOp::Eq,
        value: 200,
    };
    let outcome = evaluate(&failing, &res);
    assert!(!outcome.passed);
    assert_eq!(
        outcome.message.as_deref(),
        Some("expected status == 200, got 404")
    );
}

#[test]
fn extraction_from_fixture() {
    let res = load_fixture_response(200, 10);
    let extractors = vec![
        Extractor {
            source: ExtractorSource::JsonPath {
                expr: "$.pricing.currency".into(),
            },
            var: "currency".into(),
            scope: ExtractScope::Runtime,
            enabled: true,
        },
        Extractor {
            source: ExtractorSource::Header {
                name: "x-request-id".into(),
            },
            var: "request_id".into(),
            scope: ExtractScope::Runtime,
            enabled: true,
        },
        Extractor {
            // Regex extractors run over the (lossy) response body text.
            source: ExtractorSource::Regex {
                pattern: r#""name":\s*"(\w+)""#.into(),
                group: 1,
            },
            var: "item_name".into(),
            scope: ExtractScope::Environment,
            enabled: true,
        },
    ];

    let report = apply_extractors(&extractors, &res);
    assert!(
        report.errors.is_empty(),
        "unexpected errors: {:?}",
        report.errors
    );
    assert_eq!(
        report.values,
        vec![
            ("currency".to_string(), "USD".to_string()),
            ("request_id".to_string(), "req-77".to_string()),
            ("item_name".to_string(), "gizmo".to_string()),
        ]
    );
}
