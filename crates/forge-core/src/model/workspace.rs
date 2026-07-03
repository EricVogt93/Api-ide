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
        }
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
