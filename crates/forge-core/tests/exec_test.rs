//! Integration tests for the HTTP execution engine (`forge_core::exec`)
//! against a real (mocked) HTTP server via `wiremock`.

use std::time::Duration;

use forge_core::exec::{
    client_credentials_token, ExecError, HttpEngine, PartData, ResolvedBody, ResolvedPart,
    ResolvedRequest, TokenCache, TokenCacheKey,
};
use forge_core::model::Method;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{basic_auth, body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn get(url: impl Into<String>) -> ResolvedRequest {
    ResolvedRequest::new(Method::Get, url)
}

// ---------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------

#[tokio::test]
async fn happy_path_get_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"ok":true}"#.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let engine = HttpEngine::new();
    let req = get(format!("{}/ping", server.uri()));
    let result = engine
        .execute(req, CancellationToken::new())
        .await
        .expect("request should succeed");

    assert_eq!(result.status, 200);
    assert_eq!(result.status_text, "OK");
    assert!(result.is_json());
    assert_eq!(result.json().unwrap()["ok"], true);
    assert_eq!(result.effective_url, format!("{}/ping", server.uri()));
    assert!(result.redirect_chain.is_empty());
}

#[tokio::test]
async fn happy_path_post_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/echo"))
        .and(body_string(r#"{"a":1}"#))
        .respond_with(ResponseTemplate::new(201).set_body_string("created"))
        .mount(&server)
        .await;

    let mut req = ResolvedRequest::new(Method::Post, format!("{}/echo", server.uri()));
    req.body = ResolvedBody::Bytes {
        content_type: Some("application/json".to_string()),
        data: br#"{"a":1}"#.to_vec(),
    };

    let engine = HttpEngine::new();
    let result = engine
        .execute(req, CancellationToken::new())
        .await
        .expect("request should succeed");

    assert_eq!(result.status, 201);
    assert_eq!(result.text(), "created");
}

// ---------------------------------------------------------------------
// Header order & duplicates
// ---------------------------------------------------------------------

#[tokio::test]
async fn preserves_header_order_and_duplicates_on_the_wire() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/headers"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut req = get(format!("{}/headers", server.uri()));
    req.headers = vec![
        ("X-First".to_string(), "1".to_string()),
        ("X-Multi".to_string(), "a".to_string()),
        ("X-Second".to_string(), "2".to_string()),
        ("X-Multi".to_string(), "b".to_string()),
    ];

    let engine = HttpEngine::new();
    engine
        .execute(req, CancellationToken::new())
        .await
        .expect("request should succeed");

    // `http::HeaderMap` (used by both our request builder and wiremock's
    // capture) groups repeated header names together at their first
    // insertion point rather than preserving byte-exact interleaving with
    // *other* header names — that's a limitation of the HeaderMap type
    // itself, not something either side controls. What we *can* and do
    // guarantee: distinct header names keep their relative order, and
    // repeated values for the same name are all present, uncollapsed, in
    // their original relative order.
    let received = server.received_requests().await.expect("recording enabled");
    assert_eq!(received.len(), 1);
    let names: Vec<&str> = received[0]
        .headers
        .iter()
        .filter(|(k, _)| k.as_str().eq_ignore_ascii_case("x-first")
            || k.as_str().eq_ignore_ascii_case("x-multi")
            || k.as_str().eq_ignore_ascii_case("x-second"))
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(names, vec!["x-first", "x-multi", "x-multi", "x-second"]);

    let multi_values: Vec<&str> = received[0]
        .headers
        .get_all("x-multi")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    assert_eq!(multi_values, vec!["a", "b"]);
}

// ---------------------------------------------------------------------
// Form & multipart bodies
// ---------------------------------------------------------------------

#[tokio::test]
async fn form_body_is_url_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/form"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("name=John+Doe&tag=a%2Bb"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut req = ResolvedRequest::new(Method::Post, format!("{}/form", server.uri()));
    req.body = ResolvedBody::Form(vec![
        ("name".to_string(), "John Doe".to_string()),
        ("tag".to_string(), "a+b".to_string()),
    ]);

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await.expect("ok");
    assert_eq!(result.status, 200);
}

#[tokio::test]
async fn multipart_text_and_file_parts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, b"file contents here").expect("write temp file");

    let mut req = ResolvedRequest::new(Method::Post, format!("{}/upload", server.uri()));
    req.body = ResolvedBody::Multipart(vec![
        ResolvedPart {
            name: "field".to_string(),
            content_type: None,
            file_name: None,
            data: PartData::Text("hello world".to_string()),
        },
        ResolvedPart {
            name: "upload".to_string(),
            content_type: Some("text/plain".to_string()),
            file_name: Some("hello.txt".to_string()),
            data: PartData::File(file_path),
        },
    ]);

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await.expect("ok");
    assert_eq!(result.status, 200);

    let received = server.received_requests().await.expect("recording enabled");
    let body = String::from_utf8_lossy(&received[0].body).into_owned();
    assert!(body.contains("name=\"field\""));
    assert!(body.contains("hello world"));
    assert!(body.contains("name=\"upload\""));
    assert!(body.contains("hello.txt"));
    assert!(body.contains("file contents here"));
}

#[tokio::test]
async fn multipart_missing_file_reports_body_file_error() {
    let server = MockServer::start().await;
    let mut req = ResolvedRequest::new(Method::Post, format!("{}/upload", server.uri()));
    req.body = ResolvedBody::Multipart(vec![ResolvedPart {
        name: "upload".to_string(),
        content_type: None,
        file_name: Some("missing.bin".to_string()),
        data: PartData::File(std::path::PathBuf::from("/no/such/file/exists.bin")),
    }]);

    let engine = HttpEngine::new();
    let err = engine
        .execute(req, CancellationToken::new())
        .await
        .expect_err("missing file should error");
    assert!(matches!(err, ExecError::BodyFile { .. }));
}

// ---------------------------------------------------------------------
// Redirects
// ---------------------------------------------------------------------

#[tokio::test]
async fn follows_303_and_converts_to_get_dropping_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(303).insert_header("Location", "/next"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/next"))
        .respond_with(ResponseTemplate::new(200).set_body_string("final"))
        .mount(&server)
        .await;

    let mut req = ResolvedRequest::new(Method::Post, format!("{}/start", server.uri()));
    req.body = ResolvedBody::Bytes {
        content_type: Some("text/plain".to_string()),
        data: b"original body".to_vec(),
    };

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await.expect("ok");

    assert_eq!(result.status, 200);
    assert_eq!(result.text(), "final");
    assert_eq!(result.redirect_chain.len(), 1);
    assert_eq!(result.redirect_chain[0].status, 303);
    assert_eq!(result.redirect_chain[0].location.as_deref(), Some("/next"));
    assert_eq!(result.effective_url, format!("{}/next", server.uri()));

    // The GET to /next must not have carried a body.
    let received = server.received_requests().await.expect("recording enabled");
    let next_req = received
        .iter()
        .find(|r| r.url.path() == "/next")
        .expect("follow-up request recorded");
    assert!(next_req.body.is_empty());
    assert_eq!(next_req.method.as_str(), "GET");
}

#[tokio::test]
async fn strips_authorization_on_cross_origin_redirect() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/dest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("dest ok"))
        .mount(&target)
        .await;

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/dest", target.uri())),
        )
        .mount(&origin)
        .await;

    let mut req = get(format!("{}/start", origin.uri()));
    req.headers.push(("Authorization".to_string(), "Bearer secret-token".to_string()));
    req.headers.push(("Cookie".to_string(), "session=explicit-cookie".to_string()));

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await.expect("ok");
    assert_eq!(result.status, 200);

    let received = target.received_requests().await.expect("recording enabled");
    assert_eq!(received.len(), 1);
    assert!(received[0].headers.get("authorization").is_none());
    assert!(received[0].headers.get("cookie").is_none());
}

#[tokio::test]
async fn respects_max_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/loop1"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/loop2"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/loop2"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/loop1"))
        .mount(&server)
        .await;

    let mut req = get(format!("{}/loop1", server.uri()));
    req.max_redirects = 2;

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await.expect("ok");
    // Gives up following after max_redirects hops and returns the last
    // redirect response as final.
    assert_eq!(result.redirect_chain.len(), 2);
    assert!(matches!(result.status, 302));
}

// ---------------------------------------------------------------------
// Cookies
// ---------------------------------------------------------------------

#[tokio::test]
async fn cookie_round_trip_across_two_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/set"))
        .respond_with(
            ResponseTemplate::new(200).insert_header("set-cookie", "session=xyz789; Path=/"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/check"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let engine = HttpEngine::new();
    engine
        .execute(get(format!("{}/set", server.uri())), CancellationToken::new())
        .await
        .expect("first request ok");

    assert_eq!(engine.cookies().all().len(), 1);

    engine
        .execute(get(format!("{}/check", server.uri())), CancellationToken::new())
        .await
        .expect("second request ok");

    let received = server.received_requests().await.expect("recording enabled");
    let check_req = received.iter().find(|r| r.url.path() == "/check").unwrap();
    let cookie_header = check_req
        .headers
        .get("cookie")
        .expect("cookie header sent")
        .to_str()
        .unwrap();
    assert!(cookie_header.contains("session=xyz789"));
}

// ---------------------------------------------------------------------
// Timeout & cancellation
// ---------------------------------------------------------------------

#[tokio::test]
async fn timeout_fires_on_slow_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;

    let mut req = get(format!("{}/slow", server.uri()));
    req.timeout = Duration::from_millis(50);

    let engine = HttpEngine::new();
    let err = engine
        .execute(req, CancellationToken::new())
        .await
        .expect_err("should time out");
    assert!(matches!(err, ExecError::Timeout(_)));
}

#[tokio::test]
async fn cancellation_fires_before_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;

    let mut req = get(format!("{}/slow", server.uri()));
    req.timeout = Duration::from_secs(30);

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let engine = HttpEngine::new();
    let err = engine.execute(req, cancel).await.expect_err("should be cancelled");
    assert!(matches!(err, ExecError::Cancelled));
}

// ---------------------------------------------------------------------
// gzip decoding
// ---------------------------------------------------------------------

/// Pre-gzipped bytes for `{"gzipped":true,"value":42}`, generated offline
/// with `gzip -n` (no dependency on a compression crate at test time).
const GZIPPED_JSON: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 0, 3, 171, 86, 74, 175, 202, 44, 40, 72, 77, 81, 178, 42, 41, 42,
    77, 213, 81, 42, 75, 204, 41, 77, 85, 178, 50, 49, 170, 5, 0, 118, 207, 228, 92, 27, 0, 0, 0,
];

#[tokio::test]
async fn decodes_gzip_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .insert_header("content-type", "application/json")
                .set_body_raw(GZIPPED_JSON.to_vec(), "application/octet-stream"),
        )
        .mount(&server)
        .await;

    let engine = HttpEngine::new();
    let result = engine
        .execute(get(format!("{}/gz", server.uri())), CancellationToken::new())
        .await
        .expect("ok");

    assert_eq!(result.status, 200);
    assert_eq!(result.text(), r#"{"gzipped":true,"value":42}"#);
    assert_eq!(result.json().unwrap()["value"], 42);
}

// ---------------------------------------------------------------------
// Sizes / timing plausibility
// ---------------------------------------------------------------------

#[tokio::test]
async fn timing_and_sizes_are_plausible() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sized"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-custom", "value")
                .set_body_string("0123456789"),
        )
        .mount(&server)
        .await;

    let engine = HttpEngine::new();
    let result = engine
        .execute(get(format!("{}/sized", server.uri())), CancellationToken::new())
        .await
        .expect("ok");

    assert!(result.timing.total > Duration::ZERO);
    assert!(result.timing.ttfb + result.timing.download <= result.timing.total);
    assert_eq!(result.size.body_bytes, 10);
    assert!(result.size.header_bytes > 0);
    assert!(result.size.request_bytes > 0);
}

// ---------------------------------------------------------------------
// OAuth2 client credentials
// ---------------------------------------------------------------------

#[tokio::test]
async fn oauth_token_fetch_and_cache_hit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(basic_auth("client-id", "client-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let cache = TokenCache::new();
    let key = TokenCacheKey {
        token_url: format!("{}/token", server.uri()),
        client_id: "client-id".to_string(),
        scopes: vec![],
    };

    let first = cache
        .get_or_fetch(&client, key.clone(), "client-secret", false)
        .await
        .expect("first fetch ok");
    assert_eq!(first.access_token, "tok-1");

    // Second call should be served from cache: the mock `expect(1)` would
    // fail verification on drop if a second HTTP call were made.
    let second = cache
        .get_or_fetch(&client, key, "client-secret", false)
        .await
        .expect("second fetch (cached) ok");
    assert_eq!(second.access_token, "tok-1");

    server.verify().await;
}

#[tokio::test]
async fn oauth_credentials_in_header_vs_body() {
    let server = MockServer::start().await;
    // Header (Basic) flow.
    Mock::given(method("POST"))
        .and(path("/token-header"))
        .and(basic_auth("id1", "secret1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"header-tok","expires_in":60}"#,
        ))
        .mount(&server)
        .await;
    // Body flow: no Authorization header, credentials in the form body.
    Mock::given(method("POST"))
        .and(path("/token-body"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"body-tok","expires_in":60}"#,
        ))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();

    let header_tok = client_credentials_token(
        &client,
        &format!("{}/token-header", server.uri()),
        "id1",
        "secret1",
        &[],
        false,
    )
    .await
    .expect("header flow ok");
    assert_eq!(header_tok.access_token, "header-tok");

    let body_tok = client_credentials_token(
        &client,
        &format!("{}/token-body", server.uri()),
        "id2",
        "secret2",
        &["read".to_string(), "write".to_string()],
        true,
    )
    .await
    .expect("body flow ok");
    assert_eq!(body_tok.access_token, "body-tok");

    let received = server.received_requests().await.expect("recording enabled");
    let body_req = received
        .iter()
        .find(|r| r.url.path() == "/token-body")
        .expect("body request recorded");
    assert!(body_req.headers.get("authorization").is_none());
    let body_str = String::from_utf8_lossy(&body_req.body);
    assert!(body_str.contains("client_id=id2"));
    assert!(body_str.contains("client_secret=secret2"));
    assert!(body_str.contains("scope=read+write"));
}

#[tokio::test]
async fn oauth_error_response_surfaces_as_exec_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_client"}"#))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let err = client_credentials_token(
        &client,
        &format!("{}/token", server.uri()),
        "id",
        "secret",
        &[],
        false,
    )
    .await
    .expect_err("should fail");
    assert!(matches!(err, ExecError::OAuth(_)));
}
