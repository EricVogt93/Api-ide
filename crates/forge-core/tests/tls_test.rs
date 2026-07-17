//! Real-TLS integration tests for the HTTP engine: a rustls server with a
//! freshly generated CA proves custom-CA trust and mTLS client-certificate
//! authentication end to end.

use std::sync::Arc;

use forge_core::exec::{HttpEngine, ResolvedRequest};
use forge_core::model::Method;
use rcgen::{CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{RootCertStore, ServerConfig};
use tokio_util::sync::CancellationToken;

struct TestPki {
    ca_pem: String,
    server_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    /// Client certificate followed by its private key, as the engine expects.
    client_pem: String,
}

/// Both `ring` and `aws-lc-rs` rustls backends end up in the test dep tree,
/// so rustls can't pick a process default on its own.
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

fn make_pki() -> TestPki {
    install_crypto_provider();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().expect("ca key"))
        .expect("ca cert");

    let server_key = KeyPair::generate().expect("server key");
    let server_cert = CertificateParams::new(vec!["localhost".to_string()])
        .expect("server params")
        .signed_by(&server_key, &ca)
        .expect("server cert");

    let client_key = KeyPair::generate().expect("client key");
    let client_cert = CertificateParams::new(Vec::<String>::new())
        .expect("client params")
        .signed_by(&client_key, &ca)
        .expect("client cert");

    TestPki {
        ca_pem: ca.pem(),
        server_chain: vec![server_cert.der().clone()],
        server_key: PrivateKeyDer::from_pem_slice(server_key.serialize_pem().as_bytes())
            .expect("server key pem"),
        client_pem: format!("{}{}", client_cert.pem(), client_key.serialize_pem()),
    }
}

/// Serve exactly one HTTPS request with a canned 200, returning the bound
/// port. `require_client_cert` switches on mTLS verification against the
/// test CA.
async fn one_shot_tls_server(pki: &TestPki, require_client_cert: bool) -> u16 {
    let config = if require_client_cert {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(pki.ca_pem.as_bytes()).expect("ca der"))
            .expect("add ca");
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots)).build().expect("verifier");
        ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(pki.server_chain.clone(), pki.server_key.clone_key())
            .expect("server config")
    } else {
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(pki.server_chain.clone(), pki.server_key.clone_key())
            .expect("server config")
    };

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        let Ok(mut tls) = acceptor.accept(stream).await else { return };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = [0u8; 4096];
        let _ = tls.read(&mut buf).await;
        let _ = tls
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await;
        let _ = tls.shutdown().await;
    });

    port
}

fn https_request(port: u16) -> ResolvedRequest {
    ResolvedRequest::new(Method::Get, format!("https://localhost:{port}/"))
}

#[tokio::test]
async fn custom_ca_bundle_makes_private_ca_trusted() {
    let pki = make_pki();
    let port = one_shot_tls_server(&pki, false).await;

    let mut req = https_request(port);
    req.extra_roots_pem = Some(pki.ca_pem.clone().into_bytes());

    let engine = HttpEngine::new();
    let res = engine.execute(req, CancellationToken::new()).await.expect("request should succeed");
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"ok");
}

#[tokio::test]
async fn private_ca_is_rejected_without_the_bundle() {
    let pki = make_pki();
    let port = one_shot_tls_server(&pki, false).await;

    let engine = HttpEngine::new();
    let err = engine
        .execute(https_request(port), CancellationToken::new())
        .await
        .expect_err("unknown CA must fail TLS verification");
    let msg = err.to_string();
    assert!(
        msg.contains("certificate") || msg.contains("Connect") || msg.contains("connect"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn client_certificate_satisfies_mtls_server() {
    let pki = make_pki();
    let port = one_shot_tls_server(&pki, true).await;

    let mut req = https_request(port);
    req.extra_roots_pem = Some(pki.ca_pem.clone().into_bytes());
    req.client_pem = Some(pki.client_pem.clone().into_bytes());

    let engine = HttpEngine::new();
    let res = engine.execute(req, CancellationToken::new()).await.expect("mTLS request should succeed");
    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"ok");
}

#[tokio::test]
async fn mtls_server_rejects_requests_without_a_client_certificate() {
    let pki = make_pki();
    let port = one_shot_tls_server(&pki, true).await;

    let mut req = https_request(port);
    req.extra_roots_pem = Some(pki.ca_pem.clone().into_bytes());

    let engine = HttpEngine::new();
    let result = engine.execute(req, CancellationToken::new()).await;
    assert!(result.is_err(), "server requiring a client cert must reject the handshake");
}

#[test]
fn invalid_client_pem_yields_a_clear_error() {
    let mut req = ResolvedRequest::new(Method::Get, "https://localhost:1/");
    req.client_pem = Some(b"not a pem".to_vec());

    let engine = HttpEngine::new();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let err = rt
        .block_on(engine.execute(req, CancellationToken::new()))
        .expect_err("garbage PEM must fail");
    assert!(err.to_string().contains("client certificate"), "unexpected error: {err}");
}
