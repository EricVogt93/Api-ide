//! Turns a stored [`RequestDef`] plus variable scopes into a
//! [`ResolvedRequest`] ready for the HTTP engine: interpolation, query/path
//! params, headers, auth (including OAuth2 client-credentials token fetch)
//! and body materialization.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::prelude::*;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::exec::{
    ExecError, HttpEngine, PartData, ResolvedBody, ResolvedPart, ResolvedRequest, TokenCache,
    TokenCacheKey,
};
use crate::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, ParamKind, PartContent, RawLanguage, RequestDef,
};
use crate::store::Workspace;
use crate::vars::{interpolate, InterpolateError, VarScopes};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Interpolate(#[from] InterpolateError),
    #[error("auth: {0}")]
    Auth(String),
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error("invalid body: {0}")]
    Body(String),
}

/// Auth configs collected from the request's ancestor chain, outermost last;
/// used to resolve `AuthConfig::Inherit`.
pub type AuthChain<'a> = Vec<&'a crate::model::AuthConfig>;

/// Segment characters kept unescaped when percent-encoding a substituted
/// `:name` path parameter: everything non-alphanumeric except the small set
/// of characters that are already safe (and common) inside a path segment.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// In-memory cache of OAuth2 client-credentials tokens, shared across every
/// call to [`resolve_request`] in this process. `resolve_request` has no
/// state of its own to hang a cache off, so this lives at module scope
/// instead.
static TOKEN_CACHE: std::sync::OnceLock<TokenCache> = std::sync::OnceLock::new();

fn token_cache() -> &'static TokenCache {
    TOKEN_CACHE.get_or_init(TokenCache::new)
}

/// Plain `reqwest::Client` used only for OAuth2 token-endpoint calls.
/// [`HttpEngine`] doesn't expose its internal client (it owns cookies and a
/// pool keyed by TLS/proxy settings that aren't relevant to a token
/// request), so a small dedicated client is kept here instead.
static OAUTH_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn oauth_client() -> &'static reqwest::Client {
    OAUTH_CLIENT.get_or_init(reqwest::Client::new)
}

/// Resolve `def` into an executable request.
///
/// `engine` is needed for OAuth2 client-credentials token fetches (cached).
/// `workspace` provides root-relative body file paths and settings defaults.
pub async fn resolve_request(
    workspace: &Workspace,
    def: &RequestDef,
    auth_chain: &AuthChain<'_>,
    scopes: &VarScopes,
    engine: &HttpEngine,
) -> Result<ResolvedRequest, ResolveError> {
    // The HttpEngine doesn't expose a reusable reqwest::Client, so OAuth2
    // token fetches use their own module-level client (see `oauth_client`).
    let _ = engine;

    let mut headers: Vec<(String, String)> = Vec::new();
    for h in &def.headers {
        if !h.is_active() {
            continue;
        }
        let value = interpolate(&h.value, scopes)?;
        headers.push((h.key.clone(), value));
    }

    let auth = effective_auth(&def.auth, auth_chain);
    let (auth_headers, auth_query) = resolve_auth_additions(auth, scopes).await?;
    for (k, v) in auth_headers {
        if !headers.iter().any(|(hk, _)| hk.eq_ignore_ascii_case(&k)) {
            headers.push((k, v));
        }
    }

    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent")) {
        let ua = workspace
            .meta
            .settings
            .user_agent
            .clone()
            .unwrap_or_else(|| "Forge/0.1".to_string());
        headers.push(("User-Agent".to_string(), ua));
    }
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
        headers.push(("Accept".to_string(), "*/*".to_string()));
    }

    let url_template = interpolate(&def.url, scopes)?;
    let url_with_path = substitute_path_params(&url_template, def, scopes)?;
    let url_with_scheme = ensure_scheme(&url_with_path);
    let mut url = url::Url::parse(&url_with_scheme)
        .map_err(|e| ResolveError::Exec(ExecError::InvalidUrl(format!("{e}: {url_with_scheme}"))))?;

    let mut query_additions: Vec<(String, String)> = Vec::new();
    for p in &def.params {
        if p.kind != ParamKind::Query || !p.kv.is_active() {
            continue;
        }
        let v = interpolate(&p.kv.value, scopes)?;
        query_additions.push((p.kv.key.clone(), v));
    }
    for (k, v) in auth_query {
        // "Explicit wins": don't clobber a query param the user already
        // defined (either as an explicit `Param` row or already present in
        // the URL's own query string) with ApiKey-in-query auth.
        let already_present = query_additions.iter().any(|(qk, _)| *qk == k)
            || url.query_pairs().any(|(qk, _)| qk == k.as_str());
        if !already_present {
            query_additions.push((k, v));
        }
    }
    if !query_additions.is_empty() {
        let mut qp = url.query_pairs_mut();
        for (k, v) in &query_additions {
            qp.append_pair(k, v);
        }
    }

    let body = resolve_body(&def.body, workspace, scopes).await?;

    let ws_settings = &workspace.meta.settings;
    let timeout = Duration::from_millis(def.settings.timeout_ms.unwrap_or(ws_settings.timeout_ms));
    let follow_redirects = def.settings.follow_redirects.unwrap_or(ws_settings.follow_redirects);
    let max_redirects = def.settings.max_redirects.unwrap_or(ws_settings.max_redirects);
    let verify_tls = def.settings.verify_tls.unwrap_or(ws_settings.verify_tls);
    let proxy = ws_settings.proxy.as_ref().map(|p| p.url.clone());

    Ok(ResolvedRequest {
        method: def.method,
        url: url.to_string(),
        headers,
        body,
        timeout,
        follow_redirects,
        max_redirects,
        verify_tls,
        proxy,
    })
}

/// Interpolate `{{variables}}` in the string-valued parts of assertions
/// (header names/values, JSONPath paths and expected values, body
/// contains/matches patterns). Unlike request resolution, an unresolved
/// variable is left verbatim rather than failing the run — the request has
/// already executed by the time assertions are evaluated, and the literal
/// `{{name}}` in the failure message points straight at the typo.
pub fn resolve_assertions(
    defs: &[crate::model::AssertionDef],
    scopes: &VarScopes,
) -> Vec<crate::model::AssertionDef> {
    use crate::model::Check;

    let interp = |s: &str| interpolate(s, scopes).unwrap_or_else(|_| s.to_string());
    fn interp_json(v: &serde_json::Value, interp: &dyn Fn(&str) -> String) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => serde_json::Value::String(interp(s)),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|i| interp_json(i, interp)).collect())
            }
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter().map(|(k, val)| (k.clone(), interp_json(val, interp))).collect(),
            ),
            other => other.clone(),
        }
    }

    defs.iter()
        .map(|def| {
            let check = match &def.check {
                Check::Header { name, op, value } => {
                    Check::Header { name: interp(name), op: *op, value: interp(value) }
                }
                Check::ContentType { value } => Check::ContentType { value: interp(value) },
                Check::JsonPath { path, op, value } => Check::JsonPath {
                    path: interp(path),
                    op: *op,
                    value: interp_json(value, &interp),
                },
                Check::BodyContains { value } => Check::BodyContains { value: interp(value) },
                Check::BodyMatches { regex } => Check::BodyMatches { regex: interp(regex) },
                // No strings to interpolate; JsonSchema is deliberately left
                // untouched — schemas are structural, not user data.
                other => other.clone(),
            };
            crate::model::AssertionDef { check, enabled: def.enabled, note: def.note.clone() }
        })
        .collect()
}

/// Resolve `AuthConfig::Inherit` by walking `def_auth` then `chain`
/// (index 0 = innermost ancestor), returning the first non-`Inherit`
/// config found. `None` means no auth applies (equivalent to
/// `AuthConfig::None`).
fn effective_auth<'a>(def_auth: &'a AuthConfig, chain: &AuthChain<'a>) -> Option<&'a AuthConfig> {
    if !def_auth.is_inherit() {
        return Some(def_auth);
    }
    chain.iter().find(|a| !a.is_inherit()).copied()
}

/// Compute the headers and query params an auth config contributes.
async fn resolve_auth_additions(
    auth: Option<&AuthConfig>,
    vars: &VarScopes,
) -> Result<(Vec<(String, String)>, Vec<(String, String)>), ResolveError> {
    let Some(auth) = auth else {
        return Ok((Vec::new(), Vec::new()));
    };
    match auth {
        AuthConfig::None | AuthConfig::Inherit => Ok((Vec::new(), Vec::new())),
        AuthConfig::Basic { username, password } => {
            let user = interpolate(username, vars)?;
            let pass = interpolate(password, vars)?;
            let token = BASE64_STANDARD.encode(format!("{user}:{pass}"));
            Ok((vec![("Authorization".to_string(), format!("Basic {token}"))], Vec::new()))
        }
        AuthConfig::Bearer { token, prefix } => {
            let tok = interpolate(token, vars)?;
            let prefix = match prefix {
                Some(p) => interpolate(p, vars)?,
                None => "Bearer".to_string(),
            };
            Ok((vec![("Authorization".to_string(), format!("{prefix} {tok}"))], Vec::new()))
        }
        AuthConfig::ApiKey { key, value, placement } => {
            let k = interpolate(key, vars)?;
            let v = interpolate(value, vars)?;
            match placement {
                ApiKeyPlacement::Header => Ok((vec![(k, v)], Vec::new())),
                ApiKeyPlacement::Query => Ok((Vec::new(), vec![(k, v)])),
            }
        }
        AuthConfig::OAuth2ClientCredentials {
            token_url,
            client_id,
            client_secret,
            scopes: oauth_scopes,
            credentials_in_body,
        } => {
            let token_url = interpolate(token_url, vars)?;
            let client_id = interpolate(client_id, vars)?;
            let client_secret = interpolate(client_secret, vars)?;
            let key = TokenCacheKey { token_url, client_id, scopes: oauth_scopes.clone() };
            let token = token_cache()
                .get_or_fetch(oauth_client(), key, &client_secret, *credentials_in_body)
                .await?;
            Ok((
                vec![("Authorization".to_string(), format!("{} {}", token.token_type, token.access_token))],
                Vec::new(),
            ))
        }
        AuthConfig::OAuth2AuthCode { .. } => Err(ResolveError::Auth(
            "interactive OAuth2 authorization-code flow requires the GUI".to_string(),
        )),
    }
}

/// Substitute `:name` path segments (from `Param` rows with `kind: Path`)
/// in the already-interpolated `url`. The query string (if any) is left
/// untouched.
fn substitute_path_params(
    url: &str,
    def: &RequestDef,
    scopes: &VarScopes,
) -> Result<String, ResolveError> {
    let (path_part, query_part) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (url, None),
    };

    let mut segments: Vec<String> = Vec::new();
    for seg in path_part.split('/') {
        if let Some(name) = seg.strip_prefix(':') {
            let param = def
                .params
                .iter()
                .find(|p| p.kind == ParamKind::Path && p.kv.is_active() && p.kv.key == name);
            if let Some(param) = param {
                let value = interpolate(&param.kv.value, scopes)?;
                segments.push(utf8_percent_encode(&value, PATH_SEGMENT).to_string());
                continue;
            }
        }
        segments.push(seg.to_string());
    }

    let mut result = segments.join("/");
    if let Some(q) = query_part {
        result.push('?');
        result.push_str(q);
    }
    Ok(result)
}

/// Prepend `https://` when `url` has no scheme.
fn ensure_scheme(url: &str) -> String {
    if has_scheme(url) {
        url.to_string()
    } else {
        format!("https://{url}")
    }
}

/// Whether `url` starts with a valid URI scheme (`scheme://...`). Unlike a
/// bare `url.contains("://")` check, this only looks at the start of the
/// string, so a scheme-less URL whose *query string* happens to contain
/// `"://"` (e.g. `api.example.com/redirect?next=https://evil.com`) is
/// correctly treated as having no scheme.
fn has_scheme(url: &str) -> bool {
    let Some(idx) = url.find("://") else { return false };
    if idx == 0 {
        return false;
    }
    let scheme = &url[..idx];
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn raw_content_type(language: RawLanguage) -> &'static str {
    match language {
        RawLanguage::Text => "text/plain",
        RawLanguage::Json => "application/json",
        RawLanguage::Xml => "application/xml",
        RawLanguage::Html => "text/html",
        RawLanguage::Yaml => "application/yaml",
    }
}

/// Resolve a body-file path: absolute paths pass through, relative paths
/// are resolved against the workspace root.
fn resolve_body_path(workspace: &Workspace, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.root.join(p)
    }
}

async fn resolve_body(
    body: &BodyDef,
    workspace: &Workspace,
    scopes: &VarScopes,
) -> Result<ResolvedBody, ResolveError> {
    match body {
        BodyDef::None => Ok(ResolvedBody::None),
        BodyDef::Raw { text, language } => {
            let text = interpolate(text, scopes)?;
            Ok(ResolvedBody::Bytes {
                content_type: Some(raw_content_type(*language).to_string()),
                data: text.into_bytes(),
            })
        }
        BodyDef::Json { text } => {
            let text = interpolate(text, scopes)?;
            Ok(ResolvedBody::Bytes {
                content_type: Some("application/json".to_string()),
                data: text.into_bytes(),
            })
        }
        BodyDef::Xml { text } => {
            let text = interpolate(text, scopes)?;
            Ok(ResolvedBody::Bytes {
                content_type: Some("application/xml".to_string()),
                data: text.into_bytes(),
            })
        }
        BodyDef::FormUrlencoded { fields } => {
            let mut pairs = Vec::new();
            for f in fields {
                if !f.is_active() {
                    continue;
                }
                let v = interpolate(&f.value, scopes)?;
                pairs.push((f.key.clone(), v));
            }
            Ok(ResolvedBody::Form(pairs))
        }
        BodyDef::Multipart { parts } => {
            let mut out = Vec::new();
            for part in parts {
                if !part.enabled {
                    continue;
                }
                let (data, file_name) = match &part.content {
                    PartContent::Text { value } => (PartData::Text(interpolate(value, scopes)?), None),
                    PartContent::File { path } => {
                        let resolved = resolve_body_path(workspace, path);
                        let file_name = Path::new(path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned());
                        (PartData::File(resolved), file_name)
                    }
                };
                out.push(ResolvedPart {
                    name: part.name.clone(),
                    content_type: part.content_type.clone(),
                    file_name,
                    data,
                });
            }
            Ok(ResolvedBody::Multipart(out))
        }
        BodyDef::GraphQl { query, variables, operation_name } => {
            let query = interpolate(query, scopes)?;
            let variables_text = interpolate(variables, scopes)?;
            let variables_value: serde_json::Value = if variables_text.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&variables_text)
                    .map_err(|e| ResolveError::Body(format!("invalid GraphQL variables JSON: {e}")))?
            };
            let operation_name = match operation_name {
                Some(name) => Some(interpolate(name, scopes)?),
                None => None,
            };
            let payload = serde_json::json!({
                "query": query,
                "variables": variables_value,
                "operationName": operation_name,
            });
            let data = serde_json::to_vec(&payload)
                .map_err(|e| ResolveError::Body(format!("failed to serialize GraphQL body: {e}")))?;
            Ok(ResolvedBody::Bytes { content_type: Some("application/json".to_string()), data })
        }
        BodyDef::Binary { path } => {
            let resolved = resolve_body_path(workspace, path);
            let data = tokio::fs::read(&resolved).await.map_err(|e| {
                ResolveError::Body(format!("failed to read body file {}: {e}", resolved.display()))
            })?;
            Ok(ResolvedBody::Bytes { content_type: Some("application/octet-stream".to_string()), data })
        }
    }
}
