//! A cookie jar owned by the execution engine.
//!
//! This is a small, independent implementation over the `cookie` crate's
//! `Set-Cookie` parser — it deliberately does **not** use reqwest's built-in
//! cookie store, because Forge needs to inspect, edit and persist cookies
//! from a manager UI (`all()`, `remove()`, `to_json()`/`from_json()`), which
//! reqwest's opaque `CookieStore` trait does not support.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use url::Url;

/// A single cookie as exposed to the cookie-manager UI.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub expires: Option<DateTime<Utc>>,
    pub secure: bool,
    pub http_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CookieKey {
    domain: String,
    path: String,
    name: String,
}

#[derive(Debug, Clone)]
struct StoredEntry {
    value: String,
    expires: Option<DateTime<Utc>>,
    secure: bool,
    http_only: bool,
    /// `true` if the `Set-Cookie` response had no `Domain` attribute, in
    /// which case the cookie is a host-only cookie (RFC 6265 §5.3) and must
    /// not be sent to subdomains.
    host_only: bool,
}

/// Thread-safe cookie jar keyed by `(domain, path, name)`.
#[derive(Debug, Default)]
pub struct CookieJar {
    entries: Mutex<HashMap<CookieKey, StoredEntry>>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a raw `Set-Cookie` header value received while fetching `url`
    /// and store (or update/delete) the corresponding entry.
    pub fn store(&self, url: &Url, set_cookie_str: &str) {
        let Ok(parsed) = cookie::Cookie::parse(set_cookie_str.to_string()) else {
            return;
        };
        let name = parsed.name().to_string();
        if name.is_empty() {
            return;
        }
        let value = parsed.value().to_string();

        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let (domain, host_only) = match parsed.domain() {
            Some(d) if !d.is_empty() => (d.trim_start_matches('.').to_ascii_lowercase(), false),
            _ => (host, true),
        };
        let path = parsed
            .path()
            .filter(|p| p.starts_with('/'))
            .map(|p| p.to_string())
            .unwrap_or_else(|| default_path(url));
        let secure = parsed.secure().unwrap_or(false);
        let http_only = parsed.http_only().unwrap_or(false);

        // Max-Age takes precedence over Expires per RFC 6265 §5.3.
        let expires = if let Some(max_age) = parsed.max_age() {
            Some(Utc::now() + chrono::Duration::seconds(max_age.whole_seconds()))
        } else {
            parsed
                .expires_datetime()
                .and_then(|dt| DateTime::<Utc>::from_timestamp(dt.unix_timestamp(), 0))
        };

        let key = CookieKey { domain, path, name };
        let mut entries = self.lock_entries();
        if let Some(exp) = expires {
            if exp <= Utc::now() {
                // A cookie with an expiry in the past is a deletion request.
                entries.remove(&key);
                return;
            }
        }
        entries.insert(
            key,
            StoredEntry {
                value,
                expires,
                secure,
                http_only,
                host_only,
            },
        );
    }

    /// Cookies applicable to `url`: domain-suffix, path-prefix and secure
    /// matched, per RFC 6265 §5.4.
    pub fn matching(&self, url: &Url) -> Vec<(String, String)> {
        self.purge_expired();
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let path = url.path();
        let path = if path.is_empty() { "/" } else { path };
        let is_https = url.scheme().eq_ignore_ascii_case("https");

        let entries = self.lock_entries();
        entries
            .iter()
            .filter(|(key, entry)| {
                (!entry.secure || is_https)
                    && domain_matches(&host, &key.domain, entry.host_only)
                    && path_matches(path, &key.path)
            })
            .map(|(key, entry)| (key.name.clone(), entry.value.clone()))
            .collect()
    }

    /// All stored cookies, for the cookie-manager UI.
    pub fn all(&self) -> Vec<StoredCookie> {
        self.purge_expired();
        let entries = self.lock_entries();
        entries
            .iter()
            .map(|(key, entry)| StoredCookie {
                domain: key.domain.clone(),
                path: key.path.clone(),
                name: key.name.clone(),
                value: entry.value.clone(),
                expires: entry.expires,
                secure: entry.secure,
                http_only: entry.http_only,
            })
            .collect()
    }

    /// Remove every cookie named `name` on `domain`, regardless of path.
    pub fn remove(&self, domain: &str, name: &str) {
        let mut entries = self.lock_entries();
        entries.retain(|key, _| !(key.domain.eq_ignore_ascii_case(domain) && key.name == name));
    }

    /// Remove every stored cookie.
    pub fn clear(&self) {
        self.lock_entries().clear();
    }

    /// Serialize all cookies to a JSON array, for persistence.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.all()).unwrap_or_else(|_| "[]".to_string())
    }

    /// Rebuild a jar from JSON produced by [`Self::to_json`].
    ///
    /// Note: host-only status is not part of [`StoredCookie`], so cookies
    /// reloaded from JSON are treated as domain cookies (matched against
    /// subdomains too) even if they were originally host-only.
    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        let cookies: Vec<StoredCookie> = serde_json::from_str(data)?;
        let jar = Self::new();
        {
            let mut entries = jar.lock_entries();
            for c in cookies {
                let key = CookieKey {
                    domain: c.domain,
                    path: c.path,
                    name: c.name,
                };
                entries.insert(
                    key,
                    StoredEntry {
                        value: c.value,
                        expires: c.expires,
                        secure: c.secure,
                        http_only: c.http_only,
                        host_only: false,
                    },
                );
            }
        }
        Ok(jar)
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, HashMap<CookieKey, StoredEntry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn purge_expired(&self) {
        let now = Utc::now();
        self.lock_entries().retain(|_, entry| entry.expires.is_none_or(|e| e > now));
    }
}

/// RFC 6265 default-path algorithm.
fn default_path(url: &Url) -> String {
    let path = url.path();
    if !path.starts_with('/') || path == "/" {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => path[..idx].to_string(),
    }
}

fn domain_matches(host: &str, cookie_domain: &str, host_only: bool) -> bool {
    if host_only {
        host == cookie_domain
    } else {
        host == cookie_domain || host.ends_with(&format!(".{cookie_domain}"))
    }
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if let Some(rest) = request_path.strip_prefix(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        return rest.starts_with('/');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("valid test url")
    }

    #[test]
    fn stores_and_matches_basic_cookie() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/a/b"), "sid=abc123; Path=/a");
        let cookies = jar.matching(&url("https://example.com/a/b"));
        assert_eq!(cookies, vec![("sid".to_string(), "abc123".to_string())]);
    }

    #[test]
    fn does_not_match_different_path() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/a/b"), "sid=abc123; Path=/other");
        let cookies = jar.matching(&url("https://example.com/a/b"));
        assert!(cookies.is_empty());
    }

    #[test]
    fn host_only_cookie_does_not_match_subdomain() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "sid=abc123");
        assert!(jar.matching(&url("https://sub.example.com/")).is_empty());
        assert_eq!(
            jar.matching(&url("https://example.com/")),
            vec![("sid".to_string(), "abc123".to_string())]
        );
    }

    #[test]
    fn domain_cookie_matches_subdomain() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "sid=abc123; Domain=example.com");
        assert_eq!(
            jar.matching(&url("https://sub.example.com/")),
            vec![("sid".to_string(), "abc123".to_string())]
        );
    }

    #[test]
    fn secure_cookie_not_sent_over_http() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "sid=abc123; Secure");
        assert!(jar.matching(&url("http://example.com/")).is_empty());
        assert!(!jar.matching(&url("https://example.com/")).is_empty());
    }

    #[test]
    fn max_age_zero_deletes_cookie() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "sid=abc123");
        assert!(!jar.matching(&url("https://example.com/")).is_empty());
        jar.store(&url("https://example.com/"), "sid=abc123; Max-Age=0");
        assert!(jar.matching(&url("https://example.com/")).is_empty());
    }

    #[test]
    fn expired_cookie_is_purged() {
        let jar = CookieJar::new();
        jar.store(
            &url("https://example.com/"),
            "sid=abc123; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
        );
        assert!(jar.matching(&url("https://example.com/")).is_empty());
        assert!(jar.all().is_empty());
    }

    #[test]
    fn remove_and_clear() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "a=1");
        jar.store(&url("https://example.com/"), "b=2");
        jar.remove("example.com", "a");
        let all = jar.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "b");
        jar.clear();
        assert!(jar.all().is_empty());
    }

    #[test]
    fn json_round_trip() {
        let jar = CookieJar::new();
        jar.store(&url("https://example.com/"), "a=1; Domain=example.com; Secure; HttpOnly");
        let json = jar.to_json();
        let restored = CookieJar::from_json(&json).expect("valid json");
        let cookies = restored.matching(&url("https://example.com/"));
        assert_eq!(cookies, vec![("a".to_string(), "1".to_string())]);
        let all = restored.all();
        assert_eq!(all.len(), 1);
        assert!(all[0].secure);
        assert!(all[0].http_only);
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(CookieJar::from_json("not json").is_err());
    }
}
