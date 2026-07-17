use serde::{Deserialize, Serialize};

/// `forge.json` — the workspace root marker and global settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMeta {
    #[serde(default = "super::default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "WorkspaceSettings::is_default")]
    pub settings: WorkspaceSettings,
}

impl WorkspaceMeta {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: crate::FORMAT_VERSION,
            name: name.into(),
            settings: WorkspaceSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceSettings {
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsSettings>,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            follow_redirects: true,
            max_redirects: 10,
            verify_tls: true,
            proxy: None,
            user_agent: None,
            tls: None,
        }
    }
}

/// Client-certificate (mTLS) and trust-store settings. All paths are
/// workspace-root-relative or absolute, and point at PEM files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TlsSettings {
    /// Client certificate chain; may also contain the private key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    /// Private key, when it isn't part of `client_cert`'s file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// Extra trusted root CAs (PEM bundle), on top of the system store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_bundle: Option<String>,
}

impl TlsSettings {
    pub fn is_empty(&self) -> bool {
        self.client_cert.is_none() && self.client_key.is_none() && self.ca_bundle.is_none()
    }
}

impl WorkspaceSettings {
    pub fn is_default(&self) -> bool {
        self == &WorkspaceSettings::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// e.g. `http://127.0.0.1:8080` or `socks5://…`
    pub url: String,
    /// Comma-separated host suffixes to bypass.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub no_proxy: String,
}
