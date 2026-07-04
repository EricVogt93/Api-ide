//! End-to-end tests for `forge_core::runner` against a real (mocked) HTTP
//! server via `wiremock`, plus focused unit tests for request resolution and
//! JUnit XML rendering.

use std::collections::BTreeMap;
use std::path::PathBuf;

use forge_core::assert::AssertionOutcome;
use forge_core::exec::{ExecutionResult, HttpEngine, PartData, ResolvedBody, Sizes, TimingBreakdown};
use forge_core::model::{
    ApiKeyPlacement, AssertionDef, AuthConfig, BodyDef, Check, EnvVar, Environment, ExtractScope,
    Extractor, ExtractorSource, KeyValue, Method, MultipartPart, NumberOp, Param, ParamKind,
    PartContent, RawLanguage, RequestDef, SecretValues,
};
use forge_core::runner::{
    junit_xml, resolve_request, run, AuthChain, CancellationToken, DataSource, RequestOutcome,
    ResolveError, RunError, RunEvent, RunOptions, RunScope, RunSummary,
};
use forge_core::store::{create_collection, create_environment, create_request, save_environment, save_secrets, Workspace};
use forge_core::vars::VarScopes;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------

fn dummy_workspace() -> (tempfile::TempDir, Workspace) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = Workspace::create(dir.path(), "WS").expect("create workspace");
    (dir, ws)
}

fn write_env(root: &std::path::Path, name: &str, plain: &[(&str, &str)], secret: &[(&str, &str)]) {
    let file = create_environment(root, name).expect("create environment");
    let mut env = Environment::new(name);
    for (k, v) in plain {
        env.variables.insert((*k).to_string(), EnvVar::plain(*v));
    }
    for (k, _) in secret {
        env.variables.insert((*k).to_string(), EnvVar::secret());
    }
    save_environment(&file, &env).expect("save environment");

    let mut secrets = SecretValues::new();
    for (k, v) in secret {
        secrets.insert((*k).to_string(), (*v).to_string());
    }
    save_secrets(&file, &secrets).expect("save secrets");
}

fn sample_exec_result(status: u16, body: &[u8], total_ms: u64) -> ExecutionResult {
    ExecutionResult {
        status,
        status_text: String::new(),
        http_version: "HTTP/1.1".to_string(),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: body.to_vec(),
        timing: TimingBreakdown {
            total: std::time::Duration::from_millis(total_ms),
            ..Default::default()
        },
        size: Sizes::default(),
        effective_url: "http://example.test/".to_string(),
        redirect_chain: Vec::new(),
        cookies_set: Vec::new(),
        executed_at: chrono::Utc::now(),
    }
}

fn charge_create_def() -> RequestDef {
    let mut def = RequestDef::new("Create Charge", Method::Post, "{{baseUrl}}/charges");
    def.headers.push(KeyValue::new("X-Api-Key", "{{apiKey}}"));
    def.body = BodyDef::Json { text: "{}".to_string() };
    def.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 201 }));
    def.extractors.push(Extractor {
        source: ExtractorSource::JsonPath { expr: "$.id".to_string() },
        var: "chargeId".to_string(),
        scope: ExtractScope::Runtime,
        enabled: true,
    });
    def
}

fn charge_get_def() -> RequestDef {
    let mut def = RequestDef::new("Get Charge", Method::Get, "{{baseUrl}}/charges/{{chargeId}}");
    def.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    def
}

fn always_fails_def() -> RequestDef {
    let mut def = RequestDef::new("Should Fail", Method::Get, "{{baseUrl}}/maybe-fail");
    def.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 500 }));
    def
}

async fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

// ---------------------------------------------------------------------
// E2E: variable chaining, assertions, events, JUnit report
// ---------------------------------------------------------------------

#[tokio::test]
async fn chained_requests_extract_assert_and_report() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/charges"))
        .and(header("x-api-key", "s3cret"))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"id":"abc"}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/charges/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"abc","status":"ok"}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/maybe-fail"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[("apiKey", "s3cret")]);
    let col_dir = create_collection(root, "Payments").expect("create collection");
    create_request(&col_dir, &charge_create_def()).expect("create request A");
    create_request(&col_dir, &charge_get_def()).expect("create request B");
    create_request(&col_dir, &always_fails_def()).expect("create request C");

    let workspace = Workspace::load(root).expect("load workspace");
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let options = RunOptions { environment: Some("dev".to_string()), data: None, bail: false, delay_ms: 0 };

    let summary = run(
        &workspace,
        RunScope::Collection("collections/payments".to_string()),
        options,
        &engine,
        tx,
        CancellationToken::new(),
    )
    .await
    .expect("run ok");

    assert_eq!(summary.total, 3);
    assert_eq!(summary.passed, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 0);

    let events = drain(rx).await;
    assert!(matches!(events[0], RunEvent::RunStarted { total: 3, iterations: 1 }));
    assert!(matches!(events[1], RunEvent::IterationStarted { iteration: 0 }));

    let mut outcomes = Vec::new();
    for ev in &events {
        if let RunEvent::RequestFinished(outcome) = ev {
            outcomes.push((**outcome).clone());
        }
    }
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].name, "Create Charge");
    assert!(outcomes[0].passed(), "{:?}", outcomes[0]);
    assert_eq!(outcomes[0].extracted, vec![("chargeId".to_string(), "abc".to_string())]);

    assert_eq!(outcomes[1].name, "Get Charge");
    assert!(outcomes[1].passed(), "{:?}", outcomes[1]);

    assert_eq!(outcomes[2].name, "Should Fail");
    assert!(!outcomes[2].passed());

    assert!(matches!(events.last(), Some(RunEvent::RunFinished(_))));

    let junit = junit_xml("Payments", &outcomes, &summary);
    assert!(junit.contains("tests=\"3\""));
    assert!(junit.contains("failures=\"1\""));
    assert!(junit.contains("[iter 0] Should Fail"));
    assert!(junit.contains("<failure"));
}

// ---------------------------------------------------------------------
// E2E: data-driven CSV iterations parametrize the request path
// ---------------------------------------------------------------------

#[tokio::test]
async fn csv_data_driven_iterations_parametrize_path() {
    let server = MockServer::start().await;
    for id in ["1", "2", "3"] {
        Mock::given(method("GET"))
            .and(path(format!("/items/{id}")))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Items").expect("create collection");

    let mut def = RequestDef::new("Get Item", Method::Get, "{{baseUrl}}/items/:id");
    def.params.push(Param { kv: KeyValue::new("id", "{{id}}"), kind: ParamKind::Path });
    def.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    let file = create_request(&col_dir, &def).expect("create request");

    let workspace = Workspace::load(root).expect("load workspace");
    let rel_id = workspace.rel_id(&file);

    let csv_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/runner/items.csv"));
    let options = RunOptions {
        environment: Some("dev".to_string()),
        data: Some(DataSource::CsvFile(csv_path)),
        bail: false,
        delay_ms: 0,
    };

    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(&workspace, RunScope::Request(rel_id), options, &engine, tx, CancellationToken::new())
        .await
        .expect("run ok");

    drop(drain(rx).await);

    assert_eq!(summary.total, 3);
    assert_eq!(summary.passed, 3);
    assert_eq!(summary.failed, 0);
}

#[tokio::test]
async fn json_data_driven_iterations_parametrize_path() {
    let server = MockServer::start().await;
    for id in ["10", "20"] {
        Mock::given(method("GET"))
            .and(path(format!("/items/{id}")))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Items").expect("create collection");

    let mut def = RequestDef::new("Get Item", Method::Get, "{{baseUrl}}/items/:id");
    def.params.push(Param { kv: KeyValue::new("id", "{{id}}"), kind: ParamKind::Path });
    def.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    let file = create_request(&col_dir, &def).expect("create request");

    let workspace = Workspace::load(root).expect("load workspace");
    let rel_id = workspace.rel_id(&file);

    let json_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/runner/items.json"));
    let options = RunOptions {
        environment: Some("dev".to_string()),
        data: Some(DataSource::JsonFile(json_path)),
        bail: false,
        delay_ms: 0,
    };

    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(&workspace, RunScope::Request(rel_id), options, &engine, tx, CancellationToken::new())
        .await
        .expect("run ok");
    drop(drain(rx).await);

    assert_eq!(summary.total, 2);
    assert_eq!(summary.passed, 2);
}

#[tokio::test]
async fn runtime_vars_do_not_leak_across_iterations() {
    // Row 1 extracts `token` from the response; row 2's extraction fails
    // (non-JSON body). A second request in the same iteration reads back
    // `{{token}}` — it must only see row 1's value during row 1, and must
    // fail to resolve (not silently reuse row 1's stale value) during row 2.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/items/1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"token":"row1-token"}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/items/2"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/use")).respond_with(ResponseTemplate::new(200)).mount(&server).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Items").expect("create collection");

    let mut extract_def = RequestDef::new("Extract Token", Method::Get, "{{baseUrl}}/items/:id");
    extract_def.params.push(Param { kv: KeyValue::new("id", "{{id}}"), kind: ParamKind::Path });
    extract_def.extractors.push(Extractor {
        source: ExtractorSource::JsonPath { expr: "$.token".to_string() },
        var: "token".to_string(),
        scope: ExtractScope::Runtime,
        enabled: true,
    });
    create_request(&col_dir, &extract_def).expect("create extract request");

    let mut use_def = RequestDef::new("Use Token", Method::Get, "{{baseUrl}}/use");
    use_def.headers.push(KeyValue::new("X-Token", "{{token}}"));
    create_request(&col_dir, &use_def).expect("create use request");

    let data_path = root.join("data.json");
    std::fs::write(&data_path, r#"[{"id":"1"},{"id":"2"}]"#).expect("write data file");

    let workspace = Workspace::load(root).expect("load workspace");
    let options = RunOptions {
        environment: Some("dev".to_string()),
        data: Some(DataSource::JsonFile(data_path)),
        bail: false,
        delay_ms: 0,
    };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(
        &workspace,
        RunScope::Collection("collections/items".to_string()),
        options,
        &engine,
        tx,
        CancellationToken::new(),
    )
    .await
    .expect("run ok");

    let events = drain(rx).await;
    let mut outcomes = Vec::new();
    for ev in &events {
        if let RunEvent::RequestFinished(outcome) = ev {
            outcomes.push((**outcome).clone());
        }
    }
    assert_eq!(outcomes.len(), 4);
    assert_eq!(summary.total, 4);

    // Iteration 0: extraction succeeds, and "Use Token" resolves fine.
    assert_eq!(outcomes[0].name, "Extract Token");
    assert_eq!(outcomes[0].extracted, vec![("token".to_string(), "row1-token".to_string())]);
    assert_eq!(outcomes[1].name, "Use Token");
    assert!(outcomes[1].result.is_ok(), "{:?}", outcomes[1].result);

    // Iteration 1: extraction fails (non-JSON body), so `token` must not
    // still be set from iteration 0 — "Use Token" fails to resolve.
    assert_eq!(outcomes[2].name, "Extract Token");
    assert!(outcomes[2].extracted.is_empty());
    assert_eq!(outcomes[3].name, "Use Token");
    assert!(
        outcomes[3].result.is_err(),
        "expected row 2's Use Token to fail to resolve {{token}}, got {:?}",
        outcomes[3].result
    );
}

// ---------------------------------------------------------------------
// bail / skip_in_runs
// ---------------------------------------------------------------------

#[tokio::test]
async fn bail_stops_after_first_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/a")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Coll").expect("create collection");

    let mut a = RequestDef::new("A", Method::Get, "{{baseUrl}}/a");
    a.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    let mut b = RequestDef::new("B", Method::Get, "{{baseUrl}}/b");
    b.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    create_request(&col_dir, &a).expect("create a");
    create_request(&col_dir, &b).expect("create b");

    let workspace = Workspace::load(root).expect("load workspace");
    let options = RunOptions { environment: Some("dev".to_string()), data: None, bail: true, delay_ms: 0 };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(
        &workspace,
        RunScope::Collection("collections/coll".to_string()),
        options,
        &engine,
        tx,
        CancellationToken::new(),
    )
    .await
    .expect("run ok");
    drop(drain(rx).await);

    assert_eq!(summary.total, 2);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);

    server.verify().await;
}

#[tokio::test]
async fn bail_suppresses_iteration_started_for_fully_skipped_iterations() {
    // With `bail` and a data-driven run, once iteration 0 fails, every
    // request in iteration 1 is skipped without executing. `IterationStarted`
    // should not be emitted for an iteration that will be skipped entirely.
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/a")).respond_with(ResponseTemplate::new(500)).mount(&server).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Coll").expect("create collection");

    let mut a = RequestDef::new("A", Method::Get, "{{baseUrl}}/a");
    a.assertions.push(AssertionDef::from(Check::StatusCode { op: NumberOp::Eq, value: 200 }));
    create_request(&col_dir, &a).expect("create a");

    let data_path = root.join("data.json");
    std::fs::write(&data_path, r#"[{}, {}]"#).expect("write data file");

    let workspace = Workspace::load(root).expect("load workspace");
    let options = RunOptions {
        environment: Some("dev".to_string()),
        data: Some(DataSource::JsonFile(data_path)),
        bail: true,
        delay_ms: 0,
    };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(
        &workspace,
        RunScope::Collection("collections/coll".to_string()),
        options,
        &engine,
        tx,
        CancellationToken::new(),
    )
    .await
    .expect("run ok");

    assert_eq!(summary.total, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.skipped, 1);

    let events = drain(rx).await;
    let iteration_started_count =
        events.iter().filter(|ev| matches!(ev, RunEvent::IterationStarted { .. })).count();
    assert_eq!(iteration_started_count, 1, "iteration 1 is fully skipped and should not emit IterationStarted");
}

#[tokio::test]
async fn skip_in_runs_is_not_executed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/skip"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    write_env(root, "dev", &[("baseUrl", &server.uri())], &[]);
    let col_dir = create_collection(root, "Coll").expect("create collection");

    let mut def = RequestDef::new("Skippable", Method::Get, "{{baseUrl}}/skip");
    def.settings.skip_in_runs = true;
    let file = create_request(&col_dir, &def).expect("create request");

    let workspace = Workspace::load(root).expect("load workspace");
    let rel_id = workspace.rel_id(&file);
    let options = RunOptions { environment: Some("dev".to_string()), data: None, bail: false, delay_ms: 0 };
    let engine = HttpEngine::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let summary = run(&workspace, RunScope::Request(rel_id), options, &engine, tx, CancellationToken::new())
        .await
        .expect("run ok");
    drop(drain(rx).await);

    assert_eq!(summary.total, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.passed, 0);
    assert_eq!(summary.failed, 0);

    server.verify().await;
}

#[tokio::test]
async fn unknown_environment_is_a_run_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    Workspace::create(root, "Test WS").expect("create workspace");
    let col_dir = create_collection(root, "Coll").expect("create collection");
    create_request(&col_dir, &RequestDef::new("R", Method::Get, "https://example.com")).expect("create request");

    let workspace = Workspace::load(root).expect("load workspace");
    let options =
        RunOptions { environment: Some("nope".to_string()), data: None, bail: false, delay_ms: 0 };
    let engine = HttpEngine::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = run(&workspace, RunScope::Workspace, options, &engine, tx, CancellationToken::new())
        .await
        .expect_err("should fail: unknown environment");
    assert!(matches!(err, RunError::EnvironmentNotFound(name) if name == "nope"));
}

// ---------------------------------------------------------------------
// resolve_request: auth variants
// ---------------------------------------------------------------------

#[tokio::test]
async fn basic_auth_produces_expected_base64() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.auth = AuthConfig::Basic { username: "user".to_string(), password: "pass".to_string() };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    let auth = resolved.header("Authorization").expect("authorization header present");
    assert_eq!(auth, "Basic dXNlcjpwYXNz");
}

#[tokio::test]
async fn bearer_auth_uses_custom_prefix() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.auth = AuthConfig::Bearer { token: "tok123".to_string(), prefix: Some("Token".to_string()) };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert_eq!(resolved.header("Authorization"), Some("Token tok123"));
}

#[tokio::test]
async fn api_key_header_placement() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.auth = AuthConfig::ApiKey {
        key: "X-Key".to_string(),
        value: "secret123".to_string(),
        placement: ApiKeyPlacement::Header,
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert_eq!(resolved.header("X-Key"), Some("secret123"));
}

#[tokio::test]
async fn api_key_query_placement() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com/search");
    def.auth = AuthConfig::ApiKey {
        key: "key".to_string(),
        value: "secret123".to_string(),
        placement: ApiKeyPlacement::Query,
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    let url = url::Url::parse(&resolved.url).expect("valid url");
    assert!(url.query_pairs().any(|(k, v)| k == "key" && v == "secret123"));
}

#[tokio::test]
async fn explicit_query_param_wins_over_api_key_query_auth() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com/search");
    def.params.push(Param { kv: KeyValue::new("api_key", "user"), kind: ParamKind::Query });
    def.auth = AuthConfig::ApiKey {
        key: "api_key".to_string(),
        value: "auth-value".to_string(),
        placement: ApiKeyPlacement::Query,
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    let url = url::Url::parse(&resolved.url).expect("valid url");
    let values: Vec<_> = url.query_pairs().filter(|(k, _)| k == "api_key").map(|(_, v)| v.into_owned()).collect();
    assert_eq!(values, vec!["user".to_string()]);
}

#[tokio::test]
async fn oauth2_client_credentials_fetches_and_sets_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"tok-xyz","token_type":"Bearer","expires_in":3600}"#,
        ))
        .mount(&server)
        .await;

    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.auth = AuthConfig::OAuth2ClientCredentials {
        token_url: format!("{}/token", server.uri()),
        client_id: "cid".to_string(),
        client_secret: "csecret".to_string(),
        scopes: vec![],
        credentials_in_body: false,
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert_eq!(resolved.header("Authorization"), Some("Bearer tok-xyz"));
}

#[tokio::test]
async fn oauth2_auth_code_is_rejected_headless() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.auth = AuthConfig::OAuth2AuthCode {
        auth_url: "https://example.com/auth".to_string(),
        token_url: "https://example.com/token".to_string(),
        client_id: "cid".to_string(),
        client_secret: None,
        scopes: vec![],
        redirect_port: None,
        pkce: true,
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let err = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect_err("should fail");
    assert!(matches!(err, ResolveError::Auth(_)));
}

#[tokio::test]
async fn explicit_authorization_header_wins_over_auth_config() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com");
    def.headers.push(KeyValue::new("Authorization", "Bearer explicit-token"));
    def.auth = AuthConfig::Basic { username: "u".to_string(), password: "p".to_string() };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    let auth_headers: Vec<_> =
        resolved.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).collect();
    assert_eq!(auth_headers.len(), 1);
    assert_eq!(auth_headers[0].1, "Bearer explicit-token");
}

#[tokio::test]
async fn auth_inherit_walks_chain_to_first_concrete_config() {
    let (_dir, ws) = dummy_workspace();
    let def = RequestDef::new("r", Method::Get, "https://example.com"); // auth defaults to Inherit
    let folder_auth = AuthConfig::Inherit;
    let collection_auth = AuthConfig::Bearer { token: "col-token".to_string(), prefix: None };
    let auth_chain: AuthChain = vec![&folder_auth, &collection_auth];
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert_eq!(resolved.header("Authorization"), Some("Bearer col-token"));
}

// ---------------------------------------------------------------------
// resolve_request: URL (path params, query encoding)
// ---------------------------------------------------------------------

#[tokio::test]
async fn path_param_is_percent_encoded() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com/users/:name");
    def.params.push(Param { kv: KeyValue::new("name", "john doe"), kind: ParamKind::Path });
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert!(resolved.url.contains("/users/john%20doe"), "{}", resolved.url);
}

#[tokio::test]
async fn query_param_round_trips_special_characters() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Get, "https://example.com/search");
    def.params.push(Param { kv: KeyValue::new("q", "a b&c"), kind: ParamKind::Query });
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    let url = url::Url::parse(&resolved.url).expect("valid url");
    assert!(url.query_pairs().any(|(k, v)| k == "q" && v == "a b&c"));
}

#[tokio::test]
async fn missing_scheme_defaults_to_https() {
    let (_dir, ws) = dummy_workspace();
    let def = RequestDef::new("r", Method::Get, "example.com/ping");
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert!(resolved.url.starts_with("https://example.com/ping"), "{}", resolved.url);
}

#[tokio::test]
async fn scheme_less_url_with_scheme_looking_query_still_gets_https_prefix() {
    // The query string contains "://" but the URL itself has no scheme;
    // a naive `.contains("://")` check would wrongly treat this as already
    // having a scheme and leave it unprefixed (which then fails to parse).
    let (_dir, ws) = dummy_workspace();
    let def = RequestDef::new("r", Method::Get, "api.example.com/redirect?next=https://evil.com");
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    assert!(
        resolved.url.starts_with("https://api.example.com/redirect"),
        "{}",
        resolved.url
    );
}

// ---------------------------------------------------------------------
// resolve_request: body content types
// ---------------------------------------------------------------------

#[tokio::test]
async fn json_body_sets_content_type_and_interpolates() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Post, "https://example.com");
    def.body = BodyDef::Json { text: "{\"name\":\"{{name}}\"}".to_string() };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new().with_collection(&BTreeMap::from([("name".to_string(), "forge".to_string())]));
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Bytes { content_type, data } => {
            assert_eq!(content_type.as_deref(), Some("application/json"));
            assert_eq!(data, br#"{"name":"forge"}"#.to_vec());
        }
        other => panic!("expected Bytes body, got {other:?}"),
    }
}

#[tokio::test]
async fn raw_body_language_maps_to_content_type() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Post, "https://example.com");
    def.body = BodyDef::Raw { text: "<a/>".to_string(), language: RawLanguage::Xml };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Bytes { content_type, .. } => assert_eq!(content_type.as_deref(), Some("application/xml")),
        other => panic!("expected Bytes body, got {other:?}"),
    }
}

#[tokio::test]
async fn form_body_collects_only_enabled_fields() {
    let (_dir, ws) = dummy_workspace();
    let mut disabled = KeyValue::new("b", "2");
    disabled.enabled = false;
    let mut def = RequestDef::new("r", Method::Post, "https://example.com");
    def.body = BodyDef::FormUrlencoded { fields: vec![KeyValue::new("a", "1"), disabled] };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Form(pairs) => assert_eq!(pairs, vec![("a".to_string(), "1".to_string())]),
        other => panic!("expected Form body, got {other:?}"),
    }
}

#[tokio::test]
async fn graphql_body_builds_json_envelope() {
    let (_dir, ws) = dummy_workspace();
    let mut def = RequestDef::new("r", Method::Post, "https://example.com/graphql");
    def.body = BodyDef::GraphQl {
        query: "{ hello }".to_string(),
        variables: r#"{"x":1}"#.to_string(),
        operation_name: Some("Q".to_string()),
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Bytes { content_type, data } => {
            assert_eq!(content_type.as_deref(), Some("application/json"));
            let value: serde_json::Value = serde_json::from_slice(&data).expect("valid json");
            assert_eq!(value["query"], "{ hello }");
            assert_eq!(value["variables"]["x"], 1);
            assert_eq!(value["operationName"], "Q");
        }
        other => panic!("expected Bytes body, got {other:?}"),
    }
}

#[tokio::test]
async fn binary_body_reads_file_relative_to_workspace_root() {
    let (_dir, ws) = dummy_workspace();
    std::fs::write(ws.root.join("payload.bin"), b"hello-bytes").expect("write file");
    let mut def = RequestDef::new("r", Method::Post, "https://example.com");
    def.body = BodyDef::Binary { path: "payload.bin".to_string() };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Bytes { content_type, data } => {
            assert_eq!(content_type.as_deref(), Some("application/octet-stream"));
            assert_eq!(data, b"hello-bytes");
        }
        other => panic!("expected Bytes body, got {other:?}"),
    }
}

#[tokio::test]
async fn multipart_body_resolves_text_and_file_parts() {
    let (_dir, ws) = dummy_workspace();
    std::fs::write(ws.root.join("up.txt"), b"filedata").expect("write file");
    let mut def = RequestDef::new("r", Method::Post, "https://example.com");
    def.body = BodyDef::Multipart {
        parts: vec![
            MultipartPart {
                name: "field".to_string(),
                content: PartContent::Text { value: "hello".to_string() },
                content_type: None,
                enabled: true,
            },
            MultipartPart {
                name: "file".to_string(),
                content: PartContent::File { path: "up.txt".to_string() },
                content_type: Some("text/plain".to_string()),
                enabled: true,
            },
        ],
    };
    let engine = HttpEngine::new();
    let scopes = VarScopes::new();
    let auth_chain: AuthChain = vec![];

    let resolved = resolve_request(&ws, &def, &auth_chain, &scopes, &engine).await.expect("resolve ok");
    match resolved.body {
        ResolvedBody::Multipart(parts) => {
            assert_eq!(parts.len(), 2);
            match &parts[0].data {
                PartData::Text(t) => assert_eq!(t, "hello"),
                other => panic!("expected text part, got {other:?}"),
            }
            match &parts[1].data {
                PartData::File(p) => assert_eq!(p, &ws.root.join("up.txt")),
                other => panic!("expected file part, got {other:?}"),
            }
            assert_eq!(parts[1].file_name.as_deref(), Some("up.txt"));
        }
        other => panic!("expected Multipart body, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// JUnit XML
// ---------------------------------------------------------------------

#[test]
fn junit_xml_escapes_special_characters() {
    let outcome = RequestOutcome {
        id: "id".to_string(),
        name: "R & <B>".to_string(),
        iteration: 0,
        result: Ok(sample_exec_result(500, b"{}", 5)),
        assertions: vec![AssertionOutcome::fail(
            "check \"x\" < 5",
            "expected <5> got \"6\" & more",
        )],
        script_log: Vec::new(),
        script_error: None,
        extracted: Vec::new(),
    };
    let summary = RunSummary { total: 1, passed: 0, failed: 1, skipped: 0, duration_ms: 5 };

    let xml = junit_xml("Suite <1>", std::slice::from_ref(&outcome), &summary);

    assert!(xml.contains("Suite &lt;1&gt;"));
    assert!(xml.contains("R &amp; &lt;B&gt;"));
    assert!(xml.contains("<failure message=\"check &quot;x&quot; &lt; 5\">"));
    // Text content only needs `&`/`<`/`>` escaped, not quotes.
    assert!(xml.contains("expected &lt;5&gt; got \"6\" &amp; more"));
}

#[test]
fn junit_xml_reports_transport_errors_as_error_elements() {
    let outcome = RequestOutcome {
        id: "id".to_string(),
        name: "Broken".to_string(),
        iteration: 2,
        result: Err("connection refused".to_string()),
        assertions: Vec::new(),
        script_log: Vec::new(),
        script_error: None,
        extracted: Vec::new(),
    };
    let summary = RunSummary { total: 1, passed: 0, failed: 1, skipped: 0, duration_ms: 1 };

    let xml = junit_xml("Suite", std::slice::from_ref(&outcome), &summary);

    assert!(xml.contains("[iter 2] Broken"));
    assert!(xml.contains("<error message=\"connection refused\">connection refused</error>"));
}
