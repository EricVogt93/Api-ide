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
    let forge_core::reqv1::ResolvedBody::Json(body) = &ir.body else {
        panic!("json body")
    };
    assert_eq!(body["name"], "Alice");
    assert_eq!(body["email"], "alice@example.com");
    assert_eq!(body["tenantId"], "t-1");
    // X-Request-ID resolved from the uuid generator (a real uuid).
    let rid = ir
        .headers
        .iter()
        .find(|h| h.name == "X-Request-ID")
        .expect("request id header");
    assert_eq!(rid.value.len(), 36);
    // The secret used by the bearer hook is tracked for masking.
    assert!(ir.secret_values.contains(&"s3cr3t-token".to_string()));
}

#[test]
fn dynamic_mock_interpolation_errors_are_reported() {
    let text = r#"{
      "formatVersion": 1,
      "kind": "request",
      "meta": { "id": "mock.invalid", "name": "Invalid mock" },
      "request": { "method": "GET", "url": "${env.baseUrl}/users" },
      "mock": {
        "use": "project:mocks/create-user-response",
        "with": { "user": "${env.missing}" }
      }
    }"#;
    let doc = reqv1::RequestDocument::parse(text).expect("parse");

    let diagnostics = reqv1::validate(
        &doc,
        &project_root(),
        &request_file(),
        json!({ "baseUrl": "http://mock.local" }),
        &secret,
    )
    .expect_err("missing mock input must fail validation");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.instance_path.as_deref() == Some("/mock/with")),
        "{diagnostics:?}"
    );
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
        &doc,
        &root,
        &request_file(),
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(
        result.status,
        RunStatus::Passed,
        "diagnostics: {:?}",
        result.diagnostics
    );
    assert_eq!(result.http.as_ref().unwrap().status, 201);
    // assert-status + assert-json-path both passed.
    assert_eq!(result.assertions.len(), 2);
    assert!(
        result.assertions.iter().all(|a| a.passed),
        "{:?}",
        result.assertions
    );
    // extractor wrote userEmail into runtime.
    assert_eq!(
        result.runtime.get("userEmail"),
        Some(&Value::from("alice@example.com"))
    );
    // The secret never leaks into any diagnostic message.
    assert!(result
        .diagnostics
        .iter()
        .all(|d| !d.message.contains("s3cr3t-token")));
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
        &doc,
        &project_root(),
        &request_file(),
        env,
        &secret,
        &engine,
        RunMode::Http,
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
        &doc,
        &project_root(),
        &request_file(),
        env,
        &secret,
        &engine,
        RunMode::Mock,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(result.http.as_ref().unwrap().status, 201);
    // assert-status passed against the mock; the json-path assertion did not.
    assert!(result
        .assertions
        .iter()
        .any(|a| a.passed && a.message.contains("status")));
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
    assert!(
        errs.iter()
            .any(|d| d.instance_path.as_deref() == Some("/bindings/u")),
        "{errs:?}"
    );
}

#[test]
fn unknown_alias_is_reported() {
    let doc = reqv1::RequestDocument::parse(
        r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"},
            "bindings":{"u":{"ref":"data:does-not-exist#/x"}}}"#,
    )
    .unwrap();
    let errs =
        reqv1::validate(&doc, &project_root(), &request_file(), json!({}), &secret).unwrap_err();
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
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reqv1/schemas/request-v1.schema.json"),
    )
    .expect("fixture schema");
    assert_eq!(
        shipped, fixture,
        "fixture schema drifted from schemas/request-v1.schema.json"
    );
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
        &doc,
        &project_root(),
        &file,
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
    )
    .await
    .expect("matrix resolves");

    // Two cases in data:create-user-cases#/cases, both expect 201.
    assert_eq!(results.len(), 2);
    assert!(
        results.iter().all(|(_, r)| r.status == RunStatus::Passed),
        "{results:?}"
    );
    assert_eq!(results[0].0["case"]["name"], "valid");
    assert_eq!(results[1].0["case"]["name"], "missingEmail");

    // The server saw two distinct payloads — one per case.
    let seen = server.received_requests().await.expect("recorded");
    assert_eq!(seen.len(), 2);
    let bodies: Vec<Value> = seen
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
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
    let err =
        reqv1::matrix::resolve_cases(&doc.matrix, &resolver, &store, &root, &json!({}), &secret)
            .unwrap_err();
    assert!(
        err.0
            .iter()
            .any(|d| d.message.contains("must resolve to an array")),
        "{err:?}"
    );
}

#[tokio::test]
async fn js_assets_run_hook_assertions_extractor_and_generator() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users"))
        .and(header("authorization", "Bearer s3cr3t-token")) // JS hook
        .and(header("x-tag", "req-alice")) // JS generator binding
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "u-77", "name": "Alice"
        })))
        .mount(&server)
        .await;

    let file = project_root().join("requests/users/create-js.request.json");
    let doc = reqv1::RequestDocument::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();

    let result = reqv1::run(
        &doc,
        &project_root(),
        &file,
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(
        result.status,
        RunStatus::Passed,
        "diagnostics: {:?}",
        result.diagnostics
    );
    // The JS assertion asset returned two results, both passing.
    assert_eq!(result.assertions.len(), 2, "{:?}", result.assertions);
    assert!(result.assertions.iter().all(|a| a.passed));
    // The JS extractor wrote runtime.userId from the response body.
    assert_eq!(result.runtime.get("userId"), Some(&json!("u-77")));
}

#[tokio::test]
async fn js_dynamic_mock_serves_and_assertions_run_against_it() {
    // No HTTP server: the dynamic mock asset builds the response from the
    // bound user, so the JS assertion passes and the extractor sees u-mock.
    let file = project_root().join("requests/users/create-js.request.json");
    let doc = reqv1::RequestDocument::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let env = json!({ "baseUrl": "http://unused" });
    let engine = HttpEngine::new();

    let result = reqv1::run(
        &doc,
        &project_root(),
        &file,
        env,
        &secret,
        &engine,
        RunMode::Mock,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(
        result.status,
        RunStatus::Passed,
        "diagnostics: {:?}",
        result.diagnostics
    );
    assert_eq!(result.http.as_ref().unwrap().status, 201);
    assert_eq!(result.runtime.get("userId"), Some(&json!("u-mock")));
}

#[tokio::test]
async fn runtime_threads_from_one_request_to_the_next_in_a_sequence() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "token": "tok-xyz" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer tok-xyz")) // came from request A's extract
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "user": "alice" })))
        .mount(&server)
        .await;

    let root = project_root();
    let a = root.join("requests/users/seq-a.request.json");
    let b = root.join("requests/users/seq-b.request.json");
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();

    let results = reqv1::run_sequence(
        &[a, b],
        &root,
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].status,
        RunStatus::Passed,
        "{:?}",
        results[0].diagnostics
    );
    assert_eq!(results[0].runtime.get("authToken"), Some(&json!("tok-xyz")));
    // Request B only passes if ${runtime.authToken} reached it AND the
    // assert-schema builtin validated {user:"alice"}.
    assert_eq!(
        results[1].status,
        RunStatus::Passed,
        "{:?}",
        results[1].diagnostics
    );
    assert!(
        results[1].assertions.iter().all(|a| a.passed),
        "{:?}",
        results[1].assertions
    );
}

#[tokio::test]
async fn on_error_and_finally_phases_run() {
    // A request whose afterResponse assertion always fails is not an "error"
    // (it's Failed) — so drive an actual error via a hook that can't resolve.
    // Simplest real error: point at a dead server so the send fails.
    let doc = reqv1::RequestDocument::parse(
        r#"{
          "formatVersion": 1, "kind": "request",
          "meta": { "id": "err", "name": "err" },
          "request": { "method": "GET", "url": "${env.baseUrl}/x" },
          "pipeline": [
            { "phase": "onError", "use": "project:hooks/on-error-mark" },
            { "phase": "finally", "use": "project:hooks/finally-mark" }
          ]
        }"#,
    )
    .unwrap();

    // A port nothing listens on -> send fails -> onError + finally run.
    let env = json!({ "baseUrl": "http://127.0.0.1:9" });
    let engine = HttpEngine::new();
    // Write the doc to the fixture project so project: refs resolve.
    let file = project_root().join("requests/users/err.request.json");
    std::fs::write(&file, serde_json::to_string(&doc_json()).unwrap()).ok();

    let result = reqv1::run(
        &doc,
        &project_root(),
        &file,
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
        Value::Null,
    )
    .await;
    let _ = std::fs::remove_file(&file);

    assert_eq!(result.status, RunStatus::Error);
    // onError asset recorded the error; finally asset always ran.
    assert_eq!(result.runtime.get("errored"), Some(&json!(true)));
    assert_eq!(result.runtime.get("finallyRan"), Some(&json!(true)));
    let msg = result
        .runtime
        .get("errorMsg")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(!msg.is_empty(), "onError asset should receive ctx.error");
}

fn doc_json() -> Value {
    json!({
      "formatVersion": 1, "kind": "request",
      "meta": { "id": "err", "name": "err" },
      "request": { "method": "GET", "url": "${env.baseUrl}/x" },
      "pipeline": [
        { "phase": "onError", "use": "project:hooks/on-error-mark" },
        { "phase": "finally", "use": "project:hooks/finally-mark" }
      ]
    })
}

#[tokio::test]
async fn assert_schema_builtin_validates_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "json": { "name": "Alice", "email": "alice@example.com" }
        })))
        .mount(&server)
        .await;

    // Inline document exercising assert-schema pass and fail.
    let doc = reqv1::RequestDocument::parse(
        r#"{
          "formatVersion": 1, "kind": "request",
          "meta": { "id": "sch", "name": "sch" },
          "request": { "method": "POST", "url": "${env.baseUrl}/users",
            "headers": [ { "name": "Content-Type", "value": "application/json", "enabled": true } ],
            "body": { "type": "json", "value": { "x": 1 } } },
          "pipeline": [
            { "phase": "afterResponse", "use": "builtin:assert-schema@1",
              "with": { "schema": { "type": "object", "required": ["json"],
                "properties": { "json": { "type": "object", "required": ["name"],
                  "properties": { "name": { "type": "string" } } } } } } },
            { "phase": "afterResponse", "use": "builtin:assert-schema@1",
              "with": { "schema": { "type": "object", "required": ["missing"] } } }
          ]
        }"#,
    )
    .unwrap();
    let env = json!({ "baseUrl": server.uri() });
    let engine = HttpEngine::new();
    let result = reqv1::run(
        &doc,
        &project_root(),
        &request_file(),
        env,
        &secret,
        &engine,
        RunMode::Http,
        CancellationToken::new(),
        Value::Null,
    )
    .await;

    assert_eq!(result.assertions.len(), 2, "{:?}", result.assertions);
    assert!(
        result.assertions[0].passed,
        "matching schema should pass: {:?}",
        result.assertions[0]
    );
    assert!(
        !result.assertions[1].passed,
        "missing-required schema should fail"
    );
    assert_eq!(result.status, RunStatus::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_server_serves_over_real_http() {
    use forge_core::reqv1::MockServerConfig;

    let env = json!({ "baseUrl": "http://mock.local" });
    let config = MockServerConfig::scan(&project_root(), env, &secret).expect("scan");

    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind");
    let port = server.server_addr().to_ip().unwrap().port();

    // Serve exactly the two requests below on a blocking thread.
    std::thread::spawn(move || {
        let sec = |name: &str| (name == "apiToken").then(|| "tok".to_string());
        for _ in 0..2 {
            let request = server.recv().expect("receive");
            let method = request.method().as_str().to_string();
            let url = request.url().to_string();
            let path = url.split('?').next().unwrap_or(&url);
            match config.handle(&method, path, &sec).expect("valid mock") {
                Some(mock) => {
                    request
                        .respond(
                            tiny_http::Response::from_data(mock.body).with_status_code(mock.status),
                        )
                        .expect("respond");
                }
                None => {
                    request
                        .respond(
                            tiny_http::Response::from_string("no mock route").with_status_code(404),
                        )
                        .expect("respond");
                }
            }
        }
    });

    // Hit POST /users — a mocked route — and an unmocked one.
    let client = reqwest::Client::new();
    let ok = client
        .post(format!("http://127.0.0.1:{port}/users"))
        .send()
        .await
        .expect("request");
    assert_eq!(ok.status(), 201);
    let body: Value = ok.json().await.expect("json");
    assert!(body.get("id").is_some(), "{body}");

    let missing = client
        .get(format!("http://127.0.0.1:{port}/does-not-exist"))
        .send()
        .await
        .expect("request");
    assert_eq!(missing.status(), 404);
}
