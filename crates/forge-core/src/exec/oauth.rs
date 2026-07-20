//! OAuth2 client-credentials grant, used to fetch bearer tokens for the
//! `Auth::OAuth2ClientCredentials` request auth mode.
//!
//! This intentionally talks to the token endpoint with a plain `reqwest`
//! POST rather than the `oauth2` crate: the grant is simple enough that
//! rolling it by hand keeps it easy to test against `wiremock`, and lets us
//! reuse the caller's already-configured `reqwest::Client` (proxy, TLS
//! settings, …).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::types::ExecError;

/// A successful token endpoint response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    pub expires_in: Option<u64>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

/// Percent-encode a single `application/x-www-form-urlencoded` component;
/// mirrors [`super::engine`]'s encoder (kept local since that one is
/// private to its module).
fn form_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Run the OAuth2 "client credentials" grant against `token_url`.
///
/// When `credentials_in_body` is `false` the client id/secret are sent via
/// HTTP Basic auth (the RFC 6749 §2.3.1-recommended approach); when `true`
/// they are sent as `client_id`/`client_secret` form fields instead, for
/// servers that require it.
pub async fn client_credentials_token(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    scopes: &[String],
    credentials_in_body: bool,
) -> Result<TokenResponse, ExecError> {
    let mut form: Vec<(String, String)> =
        vec![("grant_type".to_string(), "client_credentials".to_string())];
    if !scopes.is_empty() {
        form.push(("scope".to_string(), scopes.join(" ")));
    }

    let mut builder = client.post(token_url);
    if credentials_in_body {
        form.push(("client_id".to_string(), client_id.to_string()));
        form.push(("client_secret".to_string(), client_secret.to_string()));
    } else {
        builder = builder.basic_auth(client_id, Some(client_secret));
    }

    // The `form` reqwest feature isn't enabled in this workspace, so the
    // `application/x-www-form-urlencoded` body is built by hand.
    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode_component(k), form_encode_component(v)))
        .collect::<Vec<_>>()
        .join("&");

    let response = builder
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| ExecError::OAuth(format!("request to token endpoint failed: {e}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ExecError::OAuth(format!("failed to read token response body: {e}")))?;

    if !status.is_success() {
        return Err(ExecError::OAuth(format!(
            "token endpoint returned {status}: {body}"
        )));
    }

    serde_json::from_str::<TokenResponse>(&body)
        .map_err(|e| ExecError::OAuth(format!("invalid token response ({e}): {body}")))
}

/// Cache key identifying a distinct client-credentials grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenCacheKey {
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
}

struct CachedToken {
    token: TokenResponse,
    /// `None` means the token endpoint didn't report an `expires_in`; treat
    /// as valid until explicitly invalidated.
    deadline: Option<Instant>,
}

/// Leeway subtracted from `expires_in` so a token isn't handed out right
/// before it expires mid-flight.
const EXPIRY_LEEWAY: Duration = Duration::from_secs(30);

/// In-memory cache of client-credentials tokens, keyed by endpoint/client/
/// scopes, honoring `expires_in` (minus a safety leeway).
#[derive(Default)]
pub struct TokenCache {
    entries: Mutex<HashMap<TokenCacheKey, CachedToken>>,
}

impl TokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a cached, still-valid token for `key`, or fetch (and cache) a
    /// fresh one.
    pub async fn get_or_fetch(
        &self,
        client: &reqwest::Client,
        key: TokenCacheKey,
        client_secret: &str,
        credentials_in_body: bool,
    ) -> Result<TokenResponse, ExecError> {
        if let Some(token) = self.cached(&key) {
            return Ok(token);
        }

        let token = client_credentials_token(
            client,
            &key.token_url,
            &key.client_id,
            client_secret,
            &key.scopes,
            credentials_in_body,
        )
        .await?;

        let deadline = token.expires_in.map(|secs| {
            let secs = Duration::from_secs(secs).saturating_sub(EXPIRY_LEEWAY);
            Instant::now() + secs
        });

        let mut entries = self.lock_entries();
        entries.insert(
            key,
            CachedToken {
                token: token.clone(),
                deadline,
            },
        );
        Ok(token)
    }

    /// Drop any cached token for `key`, forcing the next `get_or_fetch` to
    /// re-fetch.
    pub fn invalidate(&self, key: &TokenCacheKey) {
        self.lock_entries().remove(key);
    }

    pub fn clear(&self) {
        self.lock_entries().clear();
    }

    fn cached(&self, key: &TokenCacheKey) -> Option<TokenResponse> {
        let entries = self.lock_entries();
        let cached = entries.get(key)?;
        match cached.deadline {
            Some(deadline) if Instant::now() >= deadline => None,
            _ => Some(cached.token.clone()),
        }
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, HashMap<TokenCacheKey, CachedToken>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_defaults_token_type_to_bearer() {
        let parsed: TokenResponse =
            serde_json::from_str(r#"{"access_token":"abc","expires_in":3600}"#).unwrap();
        assert_eq!(parsed.access_token, "abc");
        assert_eq!(parsed.token_type, "Bearer");
        assert_eq!(parsed.expires_in, Some(3600));
    }

    #[test]
    fn token_response_honors_explicit_token_type() {
        let parsed: TokenResponse =
            serde_json::from_str(r#"{"access_token":"abc","token_type":"mac","expires_in":10}"#)
                .unwrap();
        assert_eq!(parsed.token_type, "mac");
    }

    #[tokio::test]
    async fn cache_miss_then_hit_without_expiry_stays_cached() {
        let cache = TokenCache::new();
        let key = TokenCacheKey {
            token_url: "http://example.invalid/token".to_string(),
            client_id: "id".to_string(),
            scopes: vec![],
        };
        // No entry yet.
        assert!(cache.cached(&key).is_none());

        // Manually seed the cache to avoid a real network call, then verify
        // a token without a deadline is always considered fresh.
        cache.lock_entries().insert(
            key.clone(),
            CachedToken {
                token: TokenResponse {
                    access_token: "tok".to_string(),
                    token_type: "Bearer".to_string(),
                    expires_in: None,
                },
                deadline: None,
            },
        );
        let cached = cache.cached(&key).expect("should be cached");
        assert_eq!(cached.access_token, "tok");
    }

    #[tokio::test]
    async fn cache_expires_after_deadline() {
        let cache = TokenCache::new();
        let key = TokenCacheKey {
            token_url: "http://example.invalid/token".to_string(),
            client_id: "id".to_string(),
            scopes: vec!["a".to_string()],
        };
        cache.lock_entries().insert(
            key.clone(),
            CachedToken {
                token: TokenResponse {
                    access_token: "tok".to_string(),
                    token_type: "Bearer".to_string(),
                    expires_in: Some(1),
                },
                deadline: Some(Instant::now() - Duration::from_secs(1)),
            },
        );
        assert!(cache.cached(&key).is_none());
    }
}
