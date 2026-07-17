//! Non-REST protocol sessions: GraphQL introspection, gRPC, WebSocket and
//! SSE.

pub mod graphql;
pub mod grpc;
pub mod sse;
pub mod websocket;

pub use graphql::{GqlField, GqlType, GraphQlSchema, INTROSPECTION_QUERY};
pub use grpc::{call, compile_protos, list_methods, GrpcError, GrpcMethod, GrpcResponse};
pub use sse::{SseEvent, SseSession};
pub use websocket::{WsEvent, WsOutgoing, WsSession};

/// Client-TLS material for protocol sessions, mirroring what the HTTP
/// engine reads from [`crate::model::TlsSettings`]: an optional client
/// certificate + key (combined PEM) and optional extra trusted roots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsMaterial {
    pub client_pem: Option<Vec<u8>>,
    pub extra_roots_pem: Option<Vec<u8>>,
}

impl TlsMaterial {
    pub fn is_empty(&self) -> bool {
        self.client_pem.is_none() && self.extra_roots_pem.is_none()
    }

    /// Read the PEM files a workspace's TLS settings point at (paths are
    /// workspace-root-relative or absolute). A separate `client_key` file
    /// is concatenated onto the cert PEM.
    pub fn from_settings(
        root: &std::path::Path,
        tls: Option<&crate::model::TlsSettings>,
    ) -> std::io::Result<Self> {
        let Some(tls) = tls else { return Ok(Self::default()) };
        let read = |p: &str| {
            let path = std::path::Path::new(p);
            let resolved = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
            std::fs::read(resolved)
        };
        let client_pem = match (&tls.client_cert, &tls.client_key) {
            (Some(cert), key) => {
                let mut pem = read(cert)?;
                if let Some(key) = key {
                    if !pem.ends_with(b"\n") {
                        pem.push(b'\n');
                    }
                    pem.extend(read(key)?);
                }
                Some(pem)
            }
            (None, _) => None,
        };
        let extra_roots_pem = match &tls.ca_bundle {
            Some(path) => Some(read(path)?),
            None => None,
        };
        Ok(Self { client_pem, extra_roots_pem })
    }
}

/// Build a rustls client config honoring `material`: webpki roots plus any
/// extra roots, and client-certificate auth when a client PEM is present.
pub(crate) fn rustls_client_config(
    material: &TlsMaterial,
) -> Result<rustls::ClientConfig, ProtocolError> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(pem) = &material.extra_roots_pem {
        for cert in CertificateDer::pem_slice_iter(pem) {
            let cert = cert.map_err(|e| {
                ProtocolError::Connect(format!("invalid CA bundle PEM: {e:?}"))
            })?;
            roots
                .add(cert)
                .map_err(|e| ProtocolError::Connect(format!("invalid CA certificate: {e}")))?;
        }
    }

    // Both the ring and aws-lc-rs rustls backends are in the dependency
    // tree, so the process default is ambiguous — pick ring explicitly.
    let builder = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| ProtocolError::Connect(format!("TLS setup failed: {e}")))?
    .with_root_certificates(roots);

    let config = match &material.client_pem {
        Some(pem) => {
            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
                .collect::<Result<_, _>>()
                .map_err(|e| {
                    ProtocolError::Connect(format!("invalid client certificate PEM: {e:?}"))
                })?;
            let key = PrivateKeyDer::from_pem_slice(pem).map_err(|e| {
                ProtocolError::Connect(format!("no private key in client PEM: {e:?}"))
            })?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| ProtocolError::Connect(format!("invalid client cert/key: {e}")))?
        }
        None => builder.with_no_client_auth(),
    };
    Ok(config)
}

/// Errors shared across the non-REST protocol adapters.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProtocolError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("{0}")]
    Http(String),
    #[error("failed to parse response: {0}")]
    Parse(String),
    #[error("connection closed")]
    Closed,
}
