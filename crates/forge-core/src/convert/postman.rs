//! Postman Collection v2.x import: collection JSON → folder/request tree,
//! plus Postman environment JSON → [`Environment`] + secret values.
//!
//! Faithful where the formats overlap (folders, requests, headers, query and
//! path params, all body modes, basic/bearer/apikey/oauth2 auth, `{{var}}`
//! syntax is shared verbatim). `pm.*` scripts come over as JavaScript
//! scripts — the engine ships a `pm` compatibility shim — with request
//! events mapping to pre/post scripts and folder/collection events to
//! `beforeEach`/`afterEach` suite hooks. What can't be mapped (saved
//! example responses, unsupported auth types) is reported in
//! [`ImportedCollection::skipped`] so the caller can show an honest summary
//! instead of pretending a lossless import.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, EnvVar, Environment, KeyValue, Method, MultipartPart,
    Param, ParamKind, PartContent, RawLanguage, RequestDef, ScriptLang, SecretValues, SuiteHooks,
};

#[derive(Debug, thiserror::Error)]
pub enum PostmanError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a Postman collection: missing info.name / item")]
    NotACollection,
    #[error("not a Postman environment: missing name / values")]
    NotAnEnvironment,
}

/// Result of parsing a Postman collection file.
#[derive(Debug)]
pub struct ImportedCollection {
    pub name: String,
    pub description: String,
    /// Collection-level `{{variables}}` (name → current/initial value).
    pub variables: BTreeMap<String, String>,
    /// Collection-level auth (`Inherit` when absent).
    pub auth: AuthConfig,
    /// Collection-level lifecycle hooks (from Postman collection events).
    pub hooks: SuiteHooks,
    pub items: Vec<ImportedItem>,
    /// Human-readable notes about dropped Postman-only features.
    pub skipped: Vec<String>,
}

impl ImportedCollection {
    /// Total number of requests across the whole tree.
    pub fn request_count(&self) -> usize {
        fn count(items: &[ImportedItem]) -> usize {
            items
                .iter()
                .map(|i| match i {
                    ImportedItem::Request(_) => 1,
                    ImportedItem::Folder { items, .. } => count(items),
                })
                .sum()
        }
        count(&self.items)
    }
}

#[derive(Debug)]
// Folder metadata outweighs the boxed request pointer; these trees are
// import-time-only and tiny, so boxing every folder isn't worth the churn.
#[allow(clippy::large_enum_variant)]
pub enum ImportedItem {
    Folder {
        name: String,
        description: String,
        auth: AuthConfig,
        hooks: SuiteHooks,
        items: Vec<ImportedItem>,
    },
    Request(Box<RequestDef>),
}

/// Parse a Postman Collection v2.0/v2.1 JSON document.
pub fn parse_postman(text: &str) -> Result<ImportedCollection, PostmanError> {
    let root: Value = serde_json::from_str(text)?;
    let info = &root["info"];
    let name = info["name"].as_str().unwrap_or_default().to_string();
    if name.is_empty() || !root["item"].is_array() {
        return Err(PostmanError::NotACollection);
    }

    let mut skipped = Vec::new();
    let hooks = hooks_from_events(&root);
    let items = parse_items(&root["item"], "", &mut skipped);

    let mut variables = BTreeMap::new();
    if let Some(vars) = root["variable"].as_array() {
        for v in vars {
            if let Some(key) = v["key"].as_str() {
                variables.insert(key.to_string(), value_as_string(&v["value"]));
            }
        }
    }

    let auth = parse_auth(&root["auth"], "collection", &mut skipped);

    Ok(ImportedCollection {
        name,
        description: description_text(&info["description"]),
        variables,
        auth,
        hooks,
        items,
        skipped,
    })
}

/// Parse a Postman environment JSON export. Variables typed `secret` come
/// back as declared-but-valueless [`EnvVar::secret`] entries with their
/// values in the separate [`SecretValues`] map (which is never committed).
pub fn parse_postman_environment(text: &str) -> Result<(Environment, SecretValues), PostmanError> {
    let root: Value = serde_json::from_str(text)?;
    let name = root["name"].as_str().unwrap_or_default();
    let Some(values) = root["values"].as_array() else {
        return Err(PostmanError::NotAnEnvironment);
    };
    if name.is_empty() {
        return Err(PostmanError::NotAnEnvironment);
    }

    let mut env = Environment::new(name);
    let mut secrets = SecretValues::new();
    for v in values {
        let Some(key) = v["key"].as_str() else {
            continue;
        };
        // Postman keeps disabled variables in the file; import them too —
        // dropping them silently would lose data the user can still see in
        // Postman's UI.
        let value = value_as_string(&v["value"]);
        if v["type"].as_str() == Some("secret") {
            env.variables.insert(key.to_string(), EnvVar::secret());
            if !value.is_empty() {
                secrets.insert(key.to_string(), value);
            }
        } else {
            env.variables.insert(key.to_string(), EnvVar::plain(value));
        }
    }
    Ok((env, secrets))
}

// ---------------------------------------------------------------------
// Item tree
// ---------------------------------------------------------------------

fn parse_items(items: &Value, path: &str, skipped: &mut Vec<String>) -> Vec<ImportedItem> {
    let Some(arr) = items.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let name = item["name"].as_str().unwrap_or("Unnamed").to_string();
        let item_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };

        if item["item"].is_array() {
            let auth = parse_auth(&item["auth"], &item_path, skipped);
            out.push(ImportedItem::Folder {
                description: description_text(&item["description"]),
                auth,
                hooks: hooks_from_events(item),
                items: parse_items(&item["item"], &item_path, skipped),
                name,
            });
        } else if item["request"].is_object() || item["request"].is_string() {
            out.push(ImportedItem::Request(Box::new(parse_request(
                item, &item_path, skipped,
            ))));
        } else {
            skipped.push(format!("{item_path}: unrecognized item, skipped"));
        }
    }
    out
}

fn parse_request(item: &Value, path: &str, skipped: &mut Vec<String>) -> RequestDef {
    let name = item["name"].as_str().unwrap_or("Unnamed").to_string();
    let req = &item["request"];

    // v2.x allows a bare string request meaning "GET <url>".
    if let Some(url) = req.as_str() {
        return RequestDef::new(name, Method::Get, url);
    }

    let method_str = req["method"].as_str().unwrap_or("GET");
    let method = Method::parse(method_str).unwrap_or_else(|| {
        skipped.push(format!(
            "{path}: unsupported method {method_str}, imported as GET"
        ));
        Method::Get
    });

    let (url, params) = parse_url(&req["url"]);
    let mut def = RequestDef::new(name, method, url);
    def.description = description_text(&req["description"]);
    def.params = params;

    if let Some(headers) = req["header"].as_array() {
        for h in headers {
            let Some(key) = h["key"].as_str() else {
                continue;
            };
            def.headers.push(KeyValue {
                key: key.to_string(),
                value: value_as_string(&h["value"]),
                description: description_text(&h["description"]),
                enabled: !h["disabled"].as_bool().unwrap_or(false),
            });
        }
    }

    def.auth = parse_auth(&req["auth"], path, skipped);
    def.body = parse_body(&req["body"], path, skipped);

    // pm.* scripts run on Forge's JS engine through the pm compatibility
    // shim, so events import as regular scripts instead of being dropped.
    def.scripts.pre_request = event_script(item, "prerequest");
    def.scripts.post_response = event_script(item, "test");
    if !def.scripts.is_empty() {
        def.scripts.language = ScriptLang::Js;
    }

    if item["response"].as_array().is_some_and(|r| !r.is_empty()) {
        skipped.push(format!("{path}: saved example responses not imported"));
    }

    def
}

/// The joined source of the first non-empty script for `listen`
/// (`prerequest` / `test`), if any. Postman stores `exec` as either an
/// array of lines or a single string.
fn event_script(item: &Value, listen: &str) -> Option<String> {
    let events = item["event"].as_array()?;
    for ev in events {
        if ev["listen"].as_str() != Some(listen) || ev["disabled"].as_bool().unwrap_or(false) {
            continue;
        }
        let exec = &ev["script"]["exec"];
        let code = match exec {
            Value::Array(lines) => lines
                .iter()
                .map(|l| l.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n"),
            Value::String(s) => s.clone(),
            _ => continue,
        };
        if !code.trim().is_empty() {
            return Some(code);
        }
    }
    None
}

/// Folder/collection events map onto suite hooks: `prerequest` runs before
/// every request underneath (→ `beforeEach`), `test` after (→ `afterEach`).
fn hooks_from_events(item: &Value) -> SuiteHooks {
    let mut hooks = SuiteHooks {
        before_each: event_script(item, "prerequest"),
        after_each: event_script(item, "test"),
        ..SuiteHooks::default()
    };
    if !hooks.is_empty() {
        hooks.language = ScriptLang::Js;
    }
    hooks
}

/// Split a Postman URL into the raw URL (query string stripped — query
/// params become explicit [`Param`] rows) plus query and path params.
fn parse_url(url: &Value) -> (String, Vec<Param>) {
    let mut params = Vec::new();

    let raw = if let Some(s) = url.as_str() {
        s.to_string()
    } else {
        let raw = url["raw"].as_str().unwrap_or_default();
        if raw.is_empty() {
            // No raw form: reconstruct from host/path segments.
            let host = join_string_array(&url["host"], ".");
            let path = join_string_array(&url["path"], "/");
            if path.is_empty() {
                host
            } else {
                format!("{host}/{path}")
            }
        } else {
            raw.to_string()
        }
    };

    if let Some(query) = url["query"].as_array() {
        for q in query {
            let Some(key) = q["key"].as_str() else {
                continue;
            };
            params.push(Param {
                kv: KeyValue {
                    key: key.to_string(),
                    value: value_as_string(&q["value"]),
                    description: description_text(&q["description"]),
                    enabled: !q["disabled"].as_bool().unwrap_or(false),
                },
                kind: ParamKind::Query,
            });
        }
    }
    if let Some(vars) = url["variable"].as_array() {
        for v in vars {
            let Some(key) = v["key"].as_str() else {
                continue;
            };
            params.push(Param {
                kv: KeyValue {
                    key: key.to_string(),
                    value: value_as_string(&v["value"]),
                    description: description_text(&v["description"]),
                    enabled: true,
                },
                kind: ParamKind::Path,
            });
        }
    }

    // Query params live in the params table; keep the URL itself clean.
    let base = raw.split('?').next().unwrap_or(&raw).to_string();
    (base, params)
}

fn parse_body(body: &Value, path: &str, skipped: &mut Vec<String>) -> BodyDef {
    if body["disabled"].as_bool().unwrap_or(false) {
        return BodyDef::None;
    }
    match body["mode"].as_str() {
        None => BodyDef::None,
        Some("raw") => {
            let text = body["raw"].as_str().unwrap_or_default().to_string();
            match body["options"]["raw"]["language"]
                .as_str()
                .unwrap_or("text")
            {
                "json" => BodyDef::Json { text },
                "xml" => BodyDef::Xml { text },
                "html" => BodyDef::Raw {
                    text,
                    language: RawLanguage::Html,
                },
                "yaml" => BodyDef::Raw {
                    text,
                    language: RawLanguage::Yaml,
                },
                _ => BodyDef::Raw {
                    text,
                    language: RawLanguage::Text,
                },
            }
        }
        Some("urlencoded") => BodyDef::FormUrlencoded {
            fields: kv_rows(&body["urlencoded"]),
        },
        Some("formdata") => {
            let mut parts = Vec::new();
            if let Some(rows) = body["formdata"].as_array() {
                for row in rows {
                    let Some(key) = row["key"].as_str() else {
                        continue;
                    };
                    let content = if row["type"].as_str() == Some("file") {
                        // `src` is a string or (multi-file) array; take the first.
                        let src = row["src"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| {
                                row["src"].as_array().and_then(|a| {
                                    a.first().and_then(Value::as_str).map(str::to_string)
                                })
                            })
                            .unwrap_or_default();
                        PartContent::File { path: src }
                    } else {
                        PartContent::Text {
                            value: value_as_string(&row["value"]),
                        }
                    };
                    parts.push(MultipartPart {
                        name: key.to_string(),
                        content,
                        content_type: row["contentType"].as_str().map(str::to_string),
                        enabled: !row["disabled"].as_bool().unwrap_or(false),
                    });
                }
            }
            BodyDef::Multipart { parts }
        }
        Some("graphql") => BodyDef::GraphQl {
            query: body["graphql"]["query"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            variables: body["graphql"]["variables"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            operation_name: None,
        },
        Some("file") => BodyDef::Binary {
            path: body["file"]["src"].as_str().unwrap_or_default().to_string(),
        },
        Some(other) => {
            skipped.push(format!(
                "{path}: unsupported body mode '{other}', body dropped"
            ));
            BodyDef::None
        }
    }
}

fn kv_rows(rows: &Value) -> Vec<KeyValue> {
    let Some(arr) = rows.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|row| {
            let key = row["key"].as_str()?;
            Some(KeyValue {
                key: key.to_string(),
                value: value_as_string(&row["value"]),
                description: description_text(&row["description"]),
                enabled: !row["disabled"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------

fn parse_auth(auth: &Value, path: &str, skipped: &mut Vec<String>) -> AuthConfig {
    let Some(kind) = auth["type"].as_str() else {
        return AuthConfig::Inherit;
    };
    let params = auth_params(&auth[kind]);
    let get = |k: &str| params.get(k).cloned().unwrap_or_default();

    match kind {
        "noauth" => AuthConfig::None,
        "basic" => AuthConfig::Basic {
            username: get("username"),
            password: get("password"),
        },
        "bearer" => AuthConfig::Bearer {
            token: get("token"),
            prefix: None,
        },
        "digest" => AuthConfig::Digest {
            username: get("username"),
            password: get("password"),
        },
        "ntlm" => AuthConfig::Ntlm {
            username: get("username"),
            password: get("password"),
            domain: get("domain"),
        },
        "awsv4" => AuthConfig::AwsSigV4 {
            access_key: get("accessKey"),
            secret_key: get("secretKey"),
            session_token: {
                let t = get("sessionToken");
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            },
            region: get("region"),
            service: get("service"),
        },
        "apikey" => AuthConfig::ApiKey {
            key: get("key"),
            value: get("value"),
            placement: if get("in") == "query" {
                ApiKeyPlacement::Query
            } else {
                ApiKeyPlacement::Header
            },
        },
        "oauth2" => {
            let scopes: Vec<String> = get("scope")
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            match get("grant_type").as_str() {
                "client_credentials" => AuthConfig::OAuth2ClientCredentials {
                    token_url: get("accessTokenUrl"),
                    client_id: get("clientId"),
                    client_secret: get("clientSecret"),
                    scopes,
                    credentials_in_body: get("client_authentication") == "body",
                },
                // Postman calls it "authorization_code" (and
                // "authorization_code_with_pkce"); both map to our
                // loopback-listener auth-code flow.
                g if g.starts_with("authorization_code") => AuthConfig::OAuth2AuthCode {
                    auth_url: get("authUrl"),
                    token_url: get("accessTokenUrl"),
                    client_id: get("clientId"),
                    client_secret: {
                        let s = get("clientSecret");
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    },
                    scopes,
                    redirect_port: None,
                    pkce: g.ends_with("with_pkce"),
                },
                other => {
                    skipped.push(format!(
                        "{path}: OAuth2 grant type '{other}' not supported, auth dropped"
                    ));
                    AuthConfig::None
                }
            }
        }
        other => {
            skipped.push(format!(
                "{path}: auth type '{other}' not supported, auth dropped"
            ));
            AuthConfig::None
        }
    }
}

/// Postman v2.1 stores auth params as `[{key, value, type}]`; v2.0 as a
/// plain object. Normalize both into a map.
fn auth_params(node: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    match node {
        Value::Array(rows) => {
            for row in rows {
                if let Some(key) = row["key"].as_str() {
                    out.insert(key.to_string(), value_as_string(&row["value"]));
                }
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                out.insert(k.clone(), value_as_string(v));
            }
        }
        _ => {}
    }
    out
}

// ---------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------

/// Postman descriptions are either a string or `{content, type}`.
fn description_text(desc: &Value) -> String {
    match desc {
        Value::String(s) => s.clone(),
        Value::Object(_) => desc["content"].as_str().unwrap_or_default().to_string(),
        _ => String::new(),
    }
}

/// Stringify a value that should be a string but may be null/number/bool.
fn value_as_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn join_string_array(v: &Value, sep: &str) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| p.as_str().unwrap_or_default())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(sep),
        _ => String::new(),
    }
}
