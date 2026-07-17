//! Shared execution contract types. These are the interface between the
//! resolve layer (which turns a `RequestDef` + variables into a
//! `ResolvedRequest`), the HTTP engine, assertions, history and the UIs.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// A request after variable interpolation and auth application —
/// everything the HTTP engine needs, nothing it has to look up.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRequest {
    pub method: crate::model::Method,
    /// Fully resolved URL including query string.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: ResolvedBody,
    pub timeout: Duration,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    /// Explicit proxy URL; `None` = use system/workspace default client.
    pub proxy: Option<String>,
    /// Client certificate + private key as a combined PEM buffer (mTLS).
    pub client_pem: Option<Vec<u8>>,
    /// Extra trusted root CAs as a PEM bundle, on top of the system store.
    pub extra_roots_pem: Option<Vec<u8>>,
}

impl ResolvedRequest {
    pub fn new(method: crate::model::Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: ResolvedBody::None,
            timeout: Duration::from_secs(30),
            follow_redirects: true,
            max_redirects: 10,
            verify_tls: true,
            proxy: None,
            client_pem: None,
            extra_roots_pem: None,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ResolvedBody {
    #[default]
    None,
    Bytes {
        content_type: Option<String>,
        data: Vec<u8>,
    },
    Form(Vec<(String, String)>),
    Multipart(Vec<ResolvedPart>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPart {
    pub name: String,
    pub content_type: Option<String>,
    pub file_name: Option<String>,
    pub data: PartData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartData {
    Text(String),
    /// Absolute path, read at send time.
    File(std::path::PathBuf),
}

/// Result of executing a request.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    /// Response headers in wire order (repeated names preserved).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timing: TimingBreakdown,
    pub size: Sizes,
    /// URL after redirects.
    pub effective_url: String,
    pub redirect_chain: Vec<Hop>,
    /// Raw `Set-Cookie` header values received.
    pub cookies_set: Vec<String>,
    pub executed_at: DateTime<Utc>,
}

impl ExecutionResult {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn is_json(&self) -> bool {
        self.content_type().is_some_and(|ct| {
            let ct = ct.to_ascii_lowercase();
            ct.contains("application/json") || ct.contains("+json")
        })
    }

    /// Body as (lossy) UTF-8 text.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Body parsed as JSON, if possible.
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Per-phase timing. reqwest cannot separate connect from TLS, so those are
/// combined in `connect_tls`; a future instrumented connector can fill the
/// separate fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimingBreakdown {
    pub dns: Option<Duration>,
    pub connect_tls: Option<Duration>,
    pub connect: Option<Duration>,
    pub tls: Option<Duration>,
    /// Send → first response byte (headers received).
    pub ttfb: Duration,
    /// Headers received → body complete.
    pub download: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sizes {
    /// Approximate request size (start line + headers + body) in bytes.
    pub request_bytes: u64,
    /// Response headers size in bytes (approximate).
    pub header_bytes: u64,
    /// Decoded response body size in bytes.
    pub body_bytes: u64,
}

/// One hop of a redirect chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub status: u16,
    pub url: String,
    pub location: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("failed to read body file {path}: {message}")]
    BodyFile { path: String, message: String },
    #[error("{0}")]
    Http(String),
    #[error("OAuth2 token request failed: {0}")]
    OAuth(String),
}
