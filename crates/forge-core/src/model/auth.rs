use serde::{Deserialize, Serialize};

/// Authentication configuration of a request, folder or collection.
///
/// `Inherit` walks up the tree (request → folder → collection) until a
/// concrete config is found; the workspace root implies `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthConfig {
    None,
    #[default]
    Inherit,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
        /// Header prefix, defaults to "Bearer".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
    ApiKey {
        key: String,
        value: String,
        #[serde(default)]
        placement: ApiKeyPlacement,
    },
    #[serde(rename_all = "camelCase")]
    OAuth2ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        /// Send credentials in the body instead of the Authorization header.
        #[serde(default, skip_serializing_if = "super::is_false")]
        credentials_in_body: bool,
    },
    #[serde(rename_all = "camelCase")]
    OAuth2AuthCode {
        auth_url: String,
        token_url: String,
        client_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        /// Loopback port for the redirect listener; 0 = ephemeral.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redirect_port: Option<u16>,
        #[serde(default = "super::default_true", skip_serializing_if = "super::is_true")]
        pkce: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyPlacement {
    #[default]
    Header,
    Query,
}

impl AuthConfig {
    pub fn is_inherit(&self) -> bool {
        matches!(self, AuthConfig::Inherit)
    }
}
