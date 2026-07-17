//! Bruno collection import: a Bruno collection is a directory tree of
//! `.bru` files (`bruno.json` marker at the root, one file per request,
//! plain subdirectories as folders, `environments/*.bru` for environments).
//!
//! The importer maps everything with a Forge equivalent — methods, URLs,
//! headers, query/path params, every body mode, basic/bearer/apikey/oauth2
//! auth, `assert` blocks (to declarative assertions), `vars:post-response`
//! extractions and `{{var}}` syntax (shared verbatim) — and reports what it
//! can't (scripts written against Bruno's `bru`/`req`/`res` JS API,
//! digest/awsv4/ntlm auth) in [`ImportedCollection::skipped`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::model::{
    ApiKeyPlacement, AssertionDef, AuthConfig, BodyDef, Check, EnvVar, Environment, ExtractScope,
    Extractor, ExtractorSource, KeyValue, Method, MultipartPart, NumberOp, Param, ParamKind,
    PartContent, RawLanguage, RequestDef, SecretValues, SuiteHooks, ValueOp,
};

use super::postman::{ImportedCollection, ImportedItem};

#[derive(Debug, thiserror::Error)]
pub enum BrunoError {
    #[error("not a Bruno collection: {0} has no bruno.json")]
    NotACollection(String),
    #[error("failed to read {path}: {message}")]
    Io { path: String, message: String },
}

/// A parsed Bruno collection: the request tree plus the collection's
/// environments (Bruno keeps them inside the collection directory).
#[derive(Debug)]
pub struct BrunoImport {
    pub collection: ImportedCollection,
    /// Environments from `environments/*.bru`. Bruno never exports secret
    /// *values* (they live in `.env`), so secrets come back declared-only.
    pub environments: Vec<(Environment, SecretValues)>,
}

/// Import the Bruno collection rooted at `root` (the directory that holds
/// `bruno.json`).
pub fn import_bruno(root: &Path) -> Result<BrunoImport, BrunoError> {
    let marker = root.join("bruno.json");
    let name = match std::fs::read_to_string(&marker) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| dir_name(root)),
        Err(_) => return Err(BrunoError::NotACollection(root.display().to_string())),
    };

    let mut skipped = Vec::new();

    // collection.bru: collection-level auth/docs (+ scripts we can't run).
    let mut auth = AuthConfig::Inherit;
    let mut description = String::new();
    let collection_bru = root.join("collection.bru");
    if let Ok(text) = std::fs::read_to_string(&collection_bru) {
        let blocks = parse_blocks(&text);
        auth = auth_from_blocks(&blocks, "collection", &mut skipped);
        description = text_block(&blocks, "docs").unwrap_or_default();
        note_scripts(&blocks, "collection", &mut skipped);
    }

    let items = read_dir_items(root, "", true, &mut skipped)?;

    let mut environments = Vec::new();
    let env_dir = root.join("environments");
    if env_dir.is_dir() {
        let mut env_files: Vec<_> = std::fs::read_dir(&env_dir)
            .map_err(|e| io_err(&env_dir, e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "bru"))
            .collect();
        env_files.sort();
        for file in env_files {
            let text = std::fs::read_to_string(&file).map_err(|e| io_err(&file, e))?;
            let env_name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            environments.push(parse_bruno_environment(&text, &env_name));
        }
    }

    Ok(BrunoImport {
        collection: ImportedCollection {
            name,
            description,
            variables: BTreeMap::new(),
            auth,
            hooks: SuiteHooks::default(),
            items,
            skipped,
        },
        environments,
    })
}

/// Parse one `environments/*.bru` file: `vars { … }` plus the
/// `vars:secret [ … ]` name list.
pub fn parse_bruno_environment(text: &str, name: &str) -> (Environment, SecretValues) {
    let blocks = parse_blocks(text);
    let mut env = Environment::new(name);
    for (key, value, _enabled) in dict_block(&blocks, "vars") {
        env.variables.insert(key, EnvVar::plain(value));
    }
    for (name, _enabled) in array_block(&blocks, "vars:secret") {
        env.variables.insert(name, EnvVar::secret());
    }
    // Bruno stores secret values outside the export, so there is nothing to
    // put into SecretValues — the declarations alone carry over.
    (env, SecretValues::new())
}

// ---------------------------------------------------------------------
// Directory tree
// ---------------------------------------------------------------------

fn read_dir_items(
    dir: &Path,
    path: &str,
    is_root: bool,
    skipped: &mut Vec<String>,
) -> Result<Vec<ImportedItem>, BrunoError> {
    // (seq, name) sort key per entry, mirroring Bruno's own ordering.
    let mut entries: Vec<(f64, String, ImportedItem)> = Vec::new();

    for entry in std::fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let p = entry.path();
        let fname = entry.file_name().to_string_lossy().into_owned();

        if p.is_dir() {
            if is_root && fname == "environments" {
                continue;
            }
            let folder_bru = p.join("folder.bru");
            let (folder_name, seq, auth, folder_desc) = match std::fs::read_to_string(&folder_bru) {
                Ok(text) => {
                    let blocks = parse_blocks(&text);
                    let meta: BTreeMap<String, String> =
                        dict_block(&blocks, "meta").into_iter().map(|(k, v, _)| (k, v)).collect();
                    let name = meta.get("name").cloned().unwrap_or_else(|| fname.clone());
                    let seq = meta.get("seq").and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::MAX);
                    let item_path = join_path(path, &name);
                    note_scripts(&blocks, &item_path, skipped);
                    let auth = auth_from_blocks(&blocks, &item_path, skipped);
                    let desc = text_block(&blocks, "docs").unwrap_or_default();
                    (name, seq, auth, desc)
                }
                Err(_) => (fname.clone(), f64::MAX, AuthConfig::Inherit, String::new()),
            };
            let item_path = join_path(path, &folder_name);
            let items = read_dir_items(&p, &item_path, false, skipped)?;
            entries.push((
                seq,
                folder_name.clone(),
                ImportedItem::Folder {
                    name: folder_name,
                    description: folder_desc,
                    auth,
                    hooks: SuiteHooks::default(),
                    items,
                },
            ));
        } else if fname.ends_with(".bru") && fname != "folder.bru" && fname != "collection.bru" {
            let text = std::fs::read_to_string(&p).map_err(|e| io_err(&p, e))?;
            let fallback = fname.trim_end_matches(".bru").to_string();
            let (def, seq) = parse_bru_request(&text, &fallback, path, skipped);
            entries.push((seq, def.name.clone(), ImportedItem::Request(Box::new(def))));
        }
    }

    entries.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(entries.into_iter().map(|(_, _, item)| item).collect())
}

// ---------------------------------------------------------------------
// Request files
// ---------------------------------------------------------------------

const METHOD_BLOCKS: [&str; 9] =
    ["get", "post", "put", "patch", "delete", "options", "head", "trace", "connect"];

/// Parse one request `.bru` file into a [`RequestDef`] plus its `meta.seq`
/// sort key.
fn parse_bru_request(
    text: &str,
    fallback_name: &str,
    parent_path: &str,
    skipped: &mut Vec<String>,
) -> (RequestDef, f64) {
    let blocks = parse_blocks(text);
    let meta: BTreeMap<String, String> =
        dict_block(&blocks, "meta").into_iter().map(|(k, v, _)| (k, v)).collect();
    let name = meta.get("name").cloned().unwrap_or_else(|| fallback_name.to_string());
    let seq = meta.get("seq").and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::MAX);
    let path = join_path(parent_path, &name);

    // The method block: `get { url: …, body: json, auth: bearer }`.
    let (method, verb_fields) = METHOD_BLOCKS
        .iter()
        .find_map(|verb| {
            blocks.iter().find(|(n, _)| n == verb).map(|(_, block)| {
                let fields: BTreeMap<String, String> =
                    block.as_dict().into_iter().map(|(k, v, _)| (k, v)).collect();
                (*verb, fields)
            })
        })
        .map(|(verb, fields)| {
            let method = Method::parse(verb).unwrap_or_else(|| {
                skipped.push(format!("{path}: unsupported method '{verb}', imported as GET"));
                Method::Get
            });
            (method, fields)
        })
        .unwrap_or_else(|| {
            skipped.push(format!("{path}: no method block found, imported as GET"));
            (Method::Get, BTreeMap::new())
        });

    let url = verb_fields.get("url").cloned().unwrap_or_default();
    let mut def = RequestDef::new(name, method, url);
    def.description = text_block(&blocks, "docs").unwrap_or_default();

    for (key, value, enabled) in dict_block(&blocks, "headers") {
        def.headers.push(KeyValue { key, value, description: String::new(), enabled });
    }
    // `query` is the legacy spelling of `params:query`.
    for block_name in ["params:query", "query"] {
        for (key, value, enabled) in dict_block(&blocks, block_name) {
            def.params.push(Param {
                kv: KeyValue { key, value, description: String::new(), enabled },
                kind: ParamKind::Query,
            });
        }
    }
    for (key, value, enabled) in dict_block(&blocks, "params:path") {
        def.params.push(Param {
            kv: KeyValue { key, value, description: String::new(), enabled },
            kind: ParamKind::Path,
        });
    }

    def.body = body_from_blocks(&blocks, verb_fields.get("body").map(String::as_str), &path, skipped);
    def.auth = match verb_fields.get("auth").map(String::as_str) {
        Some("none") => AuthConfig::None,
        Some("inherit") | None => {
            // Fall back to any auth block present even without the selector.
            auth_from_blocks(&blocks, &path, skipped)
        }
        Some(_) => auth_from_blocks(&blocks, &path, skipped),
    };

    def.assertions = assertions_from_blocks(&blocks, &path, skipped);
    def.extractors = extractors_from_blocks(&blocks, &path, skipped);
    note_scripts(&blocks, &path, skipped);

    (def, seq)
}

fn body_from_blocks(
    blocks: &[(String, Block)],
    selector: Option<&str>,
    path: &str,
    skipped: &mut Vec<String>,
) -> BodyDef {
    // The verb block's `body:` field names the active body; without it, the
    // first body block present wins.
    let want = selector.map(|s| {
        if s == "none" { "none".to_string() } else { format!("body:{s}") }
    });
    if want.as_deref() == Some("none") {
        return BodyDef::None;
    }

    let body_block = blocks.iter().find(|(n, _)| match &want {
        Some(w) => n == w || (w == "body:json" && n == "body"),
        None => n == "body" || (n.starts_with("body:") && n != "body:graphql:vars"),
    });
    let Some((name, block)) = body_block else { return BodyDef::None };

    match name.as_str() {
        "body" | "body:json" => BodyDef::Json { text: block.as_text() },
        "body:text" => BodyDef::Raw { text: block.as_text(), language: RawLanguage::Text },
        "body:sparql" => BodyDef::Raw { text: block.as_text(), language: RawLanguage::Text },
        "body:xml" => BodyDef::Xml { text: block.as_text() },
        "body:form-urlencoded" => BodyDef::FormUrlencoded {
            fields: block
                .as_dict()
                .into_iter()
                .map(|(key, value, enabled)| KeyValue { key, value, description: String::new(), enabled })
                .collect(),
        },
        "body:multipart-form" => BodyDef::Multipart {
            parts: block
                .as_dict()
                .into_iter()
                .map(|(key, value, enabled)| {
                    let (content, content_type) = parse_part_value(&value);
                    MultipartPart { name: key, content, content_type, enabled }
                })
                .collect(),
        },
        "body:graphql" => BodyDef::GraphQl {
            query: block.as_text(),
            variables: text_block(blocks, "body:graphql:vars").unwrap_or_default(),
            operation_name: None,
        },
        "body:file" => {
            let rows = block.as_dict();
            let file = rows.iter().find_map(|(_, v, _)| parse_file_ref(v));
            match file {
                Some(path) => BodyDef::Binary { path },
                None => BodyDef::None,
            }
        }
        other => {
            skipped.push(format!("{path}: unsupported body block '{other}', body dropped"));
            BodyDef::None
        }
    }
}

/// `@file(path)` with an optional trailing `@contentType(type)`.
fn parse_part_value(value: &str) -> (PartContent, Option<String>) {
    let content_type = value
        .find("@contentType(")
        .and_then(|i| value[i + "@contentType(".len()..].split(')').next())
        .map(str::to_string);
    match parse_file_ref(value) {
        Some(path) => (PartContent::File { path }, content_type),
        None => (PartContent::Text { value: value.to_string() }, content_type),
    }
}

fn parse_file_ref(value: &str) -> Option<String> {
    let start = value.find("@file(")?;
    value[start + "@file(".len()..].split(')').next().map(str::to_string)
}

fn auth_from_blocks(
    blocks: &[(String, Block)],
    path: &str,
    skipped: &mut Vec<String>,
) -> AuthConfig {
    let auth_block = blocks.iter().find(|(n, _)| n.starts_with("auth:"));
    let Some((name, block)) = auth_block else { return AuthConfig::Inherit };
    let fields: BTreeMap<String, String> =
        block.as_dict().into_iter().map(|(k, v, _)| (k, v)).collect();
    let get = |k: &str| fields.get(k).cloned().unwrap_or_default();

    match name.as_str() {
        "auth:basic" => AuthConfig::Basic { username: get("username"), password: get("password") },
        "auth:bearer" => AuthConfig::Bearer { token: get("token"), prefix: None },
        "auth:apikey" => AuthConfig::ApiKey {
            key: get("key"),
            value: get("value"),
            placement: if get("placement") == "queryparams" {
                ApiKeyPlacement::Query
            } else {
                ApiKeyPlacement::Header
            },
        },
        "auth:oauth2" => {
            let scopes: Vec<String> = get("scope")
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            match get("grant_type").as_str() {
                "client_credentials" => AuthConfig::OAuth2ClientCredentials {
                    token_url: get("access_token_url"),
                    client_id: get("client_id"),
                    client_secret: get("client_secret"),
                    scopes,
                    credentials_in_body: get("credentials_placement") == "body",
                },
                "authorization_code" => AuthConfig::OAuth2AuthCode {
                    auth_url: get("authorization_url"),
                    token_url: get("access_token_url"),
                    client_id: get("client_id"),
                    client_secret: {
                        let s = get("client_secret");
                        if s.is_empty() { None } else { Some(s) }
                    },
                    scopes,
                    redirect_port: None,
                    pkce: get("pkce") == "true",
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
                "{path}: auth type '{}' not supported, auth dropped",
                other.trim_start_matches("auth:")
            ));
            AuthConfig::None
        }
    }
}

/// Map Bruno's `assert` block (`res.status: eq 200`) onto declarative
/// assertions where an equivalent exists.
fn assertions_from_blocks(
    blocks: &[(String, Block)],
    path: &str,
    skipped: &mut Vec<String>,
) -> Vec<AssertionDef> {
    let mut out = Vec::new();
    for (expr, value, enabled) in dict_block(blocks, "assert") {
        let (op, rest) = split_op(&value);
        let check = match expr.as_str() {
            "res.status" => number_op(op).and_then(|op| {
                rest.parse::<u16>().ok().map(|value| Check::StatusCode { op, value })
            }),
            "res.responseTime" => match op {
                "lt" | "lte" => rest.parse::<u64>().ok().map(|max_ms| Check::ResponseTimeBelow { max_ms }),
                _ => None,
            },
            e if e == "res.body" || e.starts_with("res.body.") || e.starts_with("res.body[") => {
                let json_path = format!("$.{}", e.trim_start_matches("res.body").trim_start_matches('.'));
                let json_path = if json_path == "$." { "$".to_string() } else { json_path };
                value_op(op).map(|op| Check::JsonPath {
                    path: json_path,
                    op,
                    value: parse_scalar(rest),
                })
            }
            _ => None,
        };
        match check {
            Some(check) => out.push(AssertionDef { check, enabled, note: String::new() }),
            None => skipped.push(format!("{path}: assert '{expr}: {value}' has no equivalent, skipped")),
        }
    }
    out
}

/// Map `vars:post-response` entries reading from the response body
/// (`token: res.body.access_token`) onto JSONPath extractors.
fn extractors_from_blocks(
    blocks: &[(String, Block)],
    path: &str,
    skipped: &mut Vec<String>,
) -> Vec<Extractor> {
    let mut out = Vec::new();
    for (var, expr, enabled) in dict_block(blocks, "vars:post-response") {
        if expr == "res.body" || expr.starts_with("res.body.") {
            let json_path = format!("$.{}", expr.trim_start_matches("res.body").trim_start_matches('.'));
            let json_path = if json_path == "$." { "$".to_string() } else { json_path };
            out.push(Extractor {
                source: ExtractorSource::JsonPath { expr: json_path },
                var,
                scope: ExtractScope::Runtime,
                enabled,
            });
        } else {
            skipped.push(format!(
                "{path}: post-response var '{var}: {expr}' is not a res.body read, skipped"
            ));
        }
    }
    for (var, _, _) in dict_block(blocks, "vars:pre-request") {
        skipped.push(format!("{path}: pre-request var '{var}' not imported"));
    }
    out
}

fn note_scripts(blocks: &[(String, Block)], path: &str, skipped: &mut Vec<String>) {
    for name in ["script:pre-request", "script:post-response", "tests"] {
        if blocks.iter().any(|(n, b)| n == name && !b.as_text().trim().is_empty()) {
            skipped.push(format!(
                "{path}: {name} uses Bruno's bru/req/res JS API and was not imported"
            ));
        }
    }
}

// ---------------------------------------------------------------------
// Assert helpers
// ---------------------------------------------------------------------

/// Bruno's unary assert operators — they take no right-hand value.
const UNARY_OPS: [&str; 11] = [
    "isEmpty", "isNull", "isUndefined", "isDefined", "isTruthy", "isFalsy", "isJson", "isNumber",
    "isString", "isBoolean", "isArray",
];

/// Split `"eq 200"` into `("eq", "200")`. A bare unary operator keeps an
/// empty value; any other bare token is a value with an implicit `eq`.
fn split_op(value: &str) -> (&str, &str) {
    let value = value.trim();
    match value.split_once(char::is_whitespace) {
        Some((op, rest)) => (op, rest.trim()),
        None if UNARY_OPS.contains(&value) => (value, ""),
        None => ("eq", value),
    }
}

fn number_op(op: &str) -> Option<NumberOp> {
    Some(match op {
        "eq" => NumberOp::Eq,
        "neq" => NumberOp::Ne,
        "lt" => NumberOp::Lt,
        "lte" => NumberOp::Lte,
        "gt" => NumberOp::Gt,
        "gte" => NumberOp::Gte,
        _ => return None,
    })
}

fn value_op(op: &str) -> Option<ValueOp> {
    Some(match op {
        "eq" => ValueOp::Equals,
        "neq" => ValueOp::NotEquals,
        "contains" => ValueOp::Contains,
        "matches" => ValueOp::Matches,
        "isDefined" => ValueOp::Exists,
        "isUndefined" => ValueOp::NotExists,
        "lt" => ValueOp::Lt,
        "lte" => ValueOp::Lte,
        "gt" => ValueOp::Gt,
        "gte" => ValueOp::Gte,
        _ => return None,
    })
}

/// Interpret an assert value as JSON where possible (numbers, booleans),
/// falling back to a plain string.
fn parse_scalar(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

// ---------------------------------------------------------------------
// .bru block scanner
// ---------------------------------------------------------------------

/// One parsed top-level block, kept as raw lines; interpretation (dict vs
/// text vs array) is up to the consumer since e.g. `body:json` is text while
/// `headers` is a dictionary.
#[derive(Debug)]
enum Block {
    Braced(Vec<String>),
    Bracketed(Vec<String>),
}

impl Block {
    /// Dictionary view: `key: value` per line, `~` prefix = disabled.
    fn as_dict(&self) -> Vec<(String, String, bool)> {
        let Block::Braced(lines) = self else { return Vec::new() };
        lines
            .iter()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                let (key, value) = line.split_once(':')?;
                let mut key = key.trim().to_string();
                let mut enabled = true;
                if let Some(stripped) = key.strip_prefix('~') {
                    key = stripped.to_string();
                    enabled = false;
                }
                Some((key, value.trim().to_string(), enabled))
            })
            .collect()
    }

    /// Raw text view (for body/script/docs blocks): lines joined verbatim,
    /// common leading indentation removed.
    fn as_text(&self) -> String {
        let Block::Braced(lines) = self else { return String::new() };
        let indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let mut text = lines
            .iter()
            .map(|l| if l.len() >= indent { &l[indent..] } else { l.trim_start() })
            .collect::<Vec<_>>()
            .join("\n");
        while text.ends_with('\n') {
            text.pop();
        }
        text
    }

    /// Array view: one name per line/comma, `~` prefix = disabled.
    fn as_array(&self) -> Vec<(String, bool)> {
        let Block::Bracketed(lines) = self else { return Vec::new() };
        lines
            .iter()
            .flat_map(|l| l.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| match s.strip_prefix('~') {
                Some(rest) => (rest.to_string(), false),
                None => (s.to_string(), true),
            })
            .collect()
    }
}

/// Scan `.bru` text into `(name, block)` pairs. A block starts with
/// `name {` / `name [` on its own line and ends at the first `}` / `]` in
/// column 0 — Bruno's writer always formats files this way, which keeps the
/// scanner immune to braces inside indented JSON/GraphQL body content.
// ponytail: column-0 terminator heuristic; a hand-edited file with an
// unindented closing brace inside a body will end the block early.
fn parse_blocks(text: &str) -> Vec<(String, Block)> {
    let mut blocks = Vec::new();
    let mut lines = text.lines();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        let (name, bracketed) = if let Some(name) = trimmed.strip_suffix('{') {
            (name.trim(), false)
        } else if let Some(name) = trimmed.strip_suffix('[') {
            (name.trim(), true)
        } else {
            continue;
        };
        if name.is_empty() || line.starts_with(char::is_whitespace) {
            continue;
        }

        let terminator = if bracketed { "]" } else { "}" };
        let mut body: Vec<String> = Vec::new();
        for inner in lines.by_ref() {
            if inner.trim_end() == terminator {
                break;
            }
            body.push(inner.to_string());
        }
        let block = if bracketed { Block::Bracketed(body) } else { Block::Braced(body) };
        blocks.push((name.to_string(), block));
    }
    blocks
}

fn dict_block(blocks: &[(String, Block)], name: &str) -> Vec<(String, String, bool)> {
    blocks.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_dict()).unwrap_or_default()
}

fn text_block(blocks: &[(String, Block)], name: &str) -> Option<String> {
    blocks.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_text())
}

fn array_block(blocks: &[(String, Block)], name: &str) -> Vec<(String, bool)> {
    blocks.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_array()).unwrap_or_default()
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn dir_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Bruno".to_string())
}

fn io_err(path: &Path, e: std::io::Error) -> BrunoError {
    BrunoError::Io { path: path.display().to_string(), message: e.to_string() }
}
