//! Engine-level tests for challenge/signature auth: Digest (against a
//! server that verifies the RFC 7616 math independently) and AWS SigV4
//! (header shape on the wire).

use forge_core::exec::{DigestCredentials, HttpEngine, ResolvedRequest, SigV4Params};
use forge_core::model::Method;
use tokio_util::sync::CancellationToken;

const REALM: &str = "http-auth@example.org";
const NONCE: &str = "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v";
const OPAQUE: &str = "FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS";
const USER: &str = "Mufasa";
const PASS: &str = "Circle of Life";

fn md5_hex(s: &str) -> String {
    use md5::Digest as _;
    let mut h = md5::Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Pull `key="value"` / `key=value` out of a Digest Authorization header.
fn digest_param(header: &str, key: &str) -> Option<String> {
    let rest = header.trim_start().strip_prefix("Digest ")?;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{key}=")) {
            return Some(v.trim_matches('"').to_string());
        }
    }
    None
}

/// A one-shot HTTP server that answers the first request with a Digest
/// challenge and *independently verifies* the client's second request via
/// the RFC 7616 MD5 math before returning 200.
async fn digest_server() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();

            let auth_line = request
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string());

            let response = match auth_line {
                None => format!(
                    "HTTP/1.1 401 Unauthorized\r\n\
                     WWW-Authenticate: Digest realm=\"{REALM}\", qop=\"auth\", algorithm=MD5, \
                     nonce=\"{NONCE}\", opaque=\"{OPAQUE}\"\r\n\
                     content-length: 0\r\nconnection: close\r\n\r\n"
                ),
                Some(auth) => {
                    // Server-side RFC 7616 verification, computed here from
                    // scratch — not via the client's digest library.
                    let get = |k: &str| digest_param(&auth, k).unwrap_or_default();
                    let ha1 = md5_hex(&format!("{USER}:{REALM}:{PASS}"));
                    let ha2 = md5_hex(&format!("GET:{}", get("uri")));
                    let expected = md5_hex(&format!(
                        "{ha1}:{NONCE}:{}:{}:auth:{ha2}",
                        get("nc"),
                        get("cnonce")
                    ));
                    if get("response") == expected && get("username") == USER {
                        "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
                            .to_string()
                    } else {
                        "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_string()
                    }
                }
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    port
}

#[tokio::test]
async fn digest_auth_answers_the_challenge_and_passes_server_verification() {
    let port = digest_server().await;

    let mut req = ResolvedRequest::new(Method::Get, format!("http://127.0.0.1:{port}/dir/index.html"));
    req.digest = Some(DigestCredentials { username: USER.to_string(), password: PASS.to_string() });

    let engine = HttpEngine::new();
    let res = engine.execute(req, CancellationToken::new()).await.expect("request should succeed");

    assert_eq!(res.status, 200, "server rejected the computed digest");
    assert_eq!(res.body, b"ok");
}

#[tokio::test]
async fn digest_without_credentials_stays_a_plain_401() {
    let port = digest_server().await;

    let req = ResolvedRequest::new(Method::Get, format!("http://127.0.0.1:{port}/dir/index.html"));
    let engine = HttpEngine::new();
    let res = engine.execute(req, CancellationToken::new()).await.expect("request should succeed");

    assert_eq!(res.status, 401, "no credentials → the challenge is the final answer");
}

#[tokio::test]
async fn sigv4_signs_the_request_with_date_token_and_authorization() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prod/items"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut req = ResolvedRequest::new(Method::Post, format!("{}/prod/items", server.uri()));
    req.body = forge_core::exec::ResolvedBody::Bytes {
        content_type: Some("application/json".to_string()),
        data: b"{\"a\":1}".to_vec(),
    };
    req.sigv4 = Some(SigV4Params {
        access_key: "AKIDEXAMPLE".to_string(),
        secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
        session_token: Some("the-session-token".to_string()),
        region: "us-east-1".to_string(),
        service: "execute-api".to_string(),
    });

    let engine = HttpEngine::new();
    let res = engine.execute(req, CancellationToken::new()).await.expect("request should succeed");
    assert_eq!(res.status, 200);

    let received = &server.received_requests().await.expect("recorded")[0];
    let header = |name: &str| {
        received
            .headers
            .get(name)
            .map(|v| v.to_str().unwrap_or_default().to_string())
            .unwrap_or_default()
    };

    let auth = header("authorization");
    assert!(
        auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"),
        "unexpected Authorization: {auth}"
    );
    assert!(auth.contains("/us-east-1/execute-api/aws4_request"), "{auth}");
    assert!(auth.contains("SignedHeaders="), "{auth}");
    assert!(auth.to_lowercase().contains("host"), "host must be signed: {auth}");
    let signature = auth.rsplit("Signature=").next().unwrap_or_default();
    assert_eq!(signature.len(), 64, "hex sha256 signature expected: {auth}");
    assert!(signature.chars().all(|c| c.is_ascii_hexdigit()), "{auth}");

    assert_eq!(header("x-amz-date").len(), 16, "x-amz-date like 20260717T101500Z");
    assert_eq!(header("x-amz-security-token"), "the-session-token");
}
