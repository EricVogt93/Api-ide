//! End-to-end tests for the request-format v1 engine: parse the canonical
//! fixture document, resolve it to the IR (no network), run it over HTTP
//! against a wiremock server, and serve its mock.

use std::path::{Path, PathBuf};

use forge_core::exec::HttpEngine;
use forge_core::reqv1::{self, RunMode, RunStatus};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reqv1/project")
}

fn request_file() -> PathBuf {
    project_root().join("requests/users/create.request.json")
}

fn load_doc() -> reqv1::RequestDocument {
    let text = std::fs::read_to_string(request_file()).expect("read fixture");
    reqv1::RequestDocument::parse(&text).expect("fixture parses")
}

fn secret(name: &str) -> Option<String> {
    (name == "apiToken").then(|| "s3cr3t-token".to_string())
}

#[test]
fn validate_resolves_canonical_document_to_ir() {
    let doc = load_doc();
    let root = project_root();
    // Use the committed environment as-is (no network).
    let env = reqv1::load_environment(&root, Some("local")).expect("env");
    let ir = reqv1::validate(&doc, &root, &request_file(), env, &secret)
        .expect("canonical document must validate");

    assert_eq!(ir.id, "users.create");
    assert_eq!(ir.url, "http://127.0.0.1:18099/users");
    // Body variables resolved from the referenced data asset.
    let forge_core::reqv1::ResolvedBody::Json(body) = &ir.body else { panic!("json body") };
    assert_eq!(body["name"], "Alice");
    assert_eq!(body["email"], "alice@example.com");
    assert_eq!(body["tenantId"], "t-1");
    // X-Request-ID resolved from the uuid generator (a real uuid).
    let rid = ir.headers.iter().find(|h| h.name == "X-Request-ID").expect("request id header");
    assert_eq!(rid.value.len(), 36);
    // The secret used by the bearer hook is tracked for masking.
    assert!(ir.secret_values.contains(&"s3cr3t-token".to_string()));
}

#[tokio::test]
async fn runs_over_http_with_hooks_assertions_and_extractor() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users"))
        .and(header("authorization", "Bearer s3cr3t-token")) // beforeRequest bearer hook
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "json": { "name": "Alice", "email": "alice@example.com" }
        })))
        .mount(&server)
        .await;

    let doc = load_doc();
    let root = project_root();
    // Point the environment's baseUrl at the mock server.
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();

    let result = reqv1::run(
        &doc, &root, &request_file(), env, &secret, &engine, RunMode::Http, CancellationToken::new(), Value::Null,
    )
    .await;

    assert_eq!(result.status, RunStatus::Passed, "diagnostics: {:?}", result.diagnostics);
    assert_eq!(result.http.as_ref().unwrap().status, 201);
    // assert-status + assert-json-path both passed.
    assert_eq!(result.assertions.len(), 2);
    assert!(result.assertions.iter().all(|a| a.passed), "{:?}", result.assertions);
    // extractor wrote userEmail into runtime.
    assert_eq!(result.runtime.get("userEmail"), Some(&Value::from("alice@example.com")));
    // The secret never leaks into any diagnostic message.
    assert!(result.diagnostics.iter().all(|d| !d.message.contains("s3cr3t-token")));
}

#[tokio::test]
async fn failed_assertion_marks_run_failed() {
    let server = MockServer::start().await;
    // Return the wrong name so assert-json-path fails.
    Mock::given(method("POST"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "json": { "name": "Bob", "email": "bob@example.com" }
        })))
        .mount(&server)
        .await;

    let doc = load_doc();
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();
    let result = reqv1::run(
        &doc, &project_root(), &request_file(), env, &secret, &engine, RunMode::Http,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(result.status, RunStatus::Failed);
    assert!(result.assertions.iter().any(|a| !a.passed));
}

#[tokio::test]
async fn mock_mode_serves_the_document_mock_and_runs_after_response() {
    // No server: mock mode replaces the send. The document's mock returns 201
    // + data:user-responses#/created ({id:u-1,name:Alice}). assert-status
    // passes; assert-json-path on $.json.name fails (mock body has no `json`
    // wrapper) — proving assertions run against the mock, catching drift.
    let doc = load_doc();
    let env = json!({ "baseUrl": "http://unused" });
    let engine = HttpEngine::new();
    let result = reqv1::run(
        &doc, &project_root(), &request_file(), env, &secret, &engine, RunMode::Mock,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(result.http.as_ref().unwrap().status, 201);
    // assert-status passed against the mock; the json-path assertion did not.
    assert!(result.assertions.iter().any(|a| a.passed && a.message.contains("status")));
    assert!(result.assertions.iter().any(|a| !a.passed));
}

#[test]
fn missing_asset_is_reported_with_pointer() {
    let doc = reqv1::RequestDocument::parse(
        r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"${env.baseUrl}/x"},
            "bindings":{"u":{"ref":"data:users#/valid/nobody"}}}"#,
    )
    .unwrap();
    let root = project_root();
    let env = json!({ "baseUrl": "http://x" });
    let errs = reqv1::validate(&doc, &root, &request_file(), env, &secret).unwrap_err();
    assert!(errs.iter().any(|d| d.code == "INVALID_POINTER"), "{errs:?}");
    assert!(errs.iter().any(|d| d.instance_path.as_deref() == Some("/bindings/u")), "{errs:?}");
}

#[test]
fn unknown_alias_is_reported() {
    let doc = reqv1::RequestDocument::parse(
        r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"},
            "bindings":{"u":{"ref":"data:does-not-exist#/x"}}}"#,
    )
    .unwrap();
    let errs = reqv1::validate(&doc, &project_root(), &request_file(), json!({}), &secret).unwrap_err();
    assert!(errs.iter().any(|d| d.code == "INVALID_ALIAS"), "{errs:?}");
}

#[test]
fn schema_json_matches_the_shipped_schema() {
    // The fixture copy and the source schema must not drift.
    let shipped = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/request-v1.schema.json"),
    )
    .expect("shipped schema");
    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reqv1/schemas/request-v1.schema.json"),
    )
    .expect("fixture schema");
    assert_eq!(shipped, fixture, "fixture schema drifted from schemas/request-v1.schema.json");
}

#[tokio::test]
async fn matrix_runs_once_per_case_with_case_scoped_values() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let file = project_root().join("requests/users/create-cases.request.json");
    let text = std::fs::read_to_string(&file).expect("read fixture");
    let doc = reqv1::RequestDocument::parse(&text).expect("parses");
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();

    let results = reqv1::run_matrix(
        &doc, &project_root(), &file, env, &secret, &engine, RunMode::Http,
        CancellationToken::new(),
    )
    .await
    .expect("matrix resolves");

    // Two cases in data:create-user-cases#/cases, both expect 201.
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|(_, r)| r.status == RunStatus::Passed), "{results:?}");
    assert_eq!(results[0].0["case"]["name"], "valid");
    assert_eq!(results[1].0["case"]["name"], "missingEmail");

    // The server saw two distinct payloads — one per case.
    let seen = server.received_requests().await.expect("recorded");
    assert_eq!(seen.len(), 2);
    let bodies: Vec<Value> =
        seen.iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect();
    assert!(bodies.contains(&json!({ "name": "Alice" })));
    assert!(bodies.contains(&json!({ "name": "Bob" })));
}

#[test]
fn matrix_binding_must_be_an_array() {
    let doc = reqv1::RequestDocument::parse(
        r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "matrix":{"case":{"value":42}},
            "request":{"method":"GET","url":"http://x"}}"#,
    )
    .unwrap();
    let root = project_root();
    let project = reqv1::load_project(&root).unwrap();
    let resolver = reqv1::RefResolver::new(&root, &project).unwrap();
    let store = reqv1::DataStore::new(&resolver);
    let err = reqv1::matrix::resolve_cases(
        &doc.matrix, &resolver, &store, &root, &json!({}), &secret,
    )
    .unwrap_err();
    assert!(err.0.iter().any(|d| d.message.contains("must resolve to an array")), "{err:?}");
}
