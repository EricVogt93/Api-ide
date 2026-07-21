//! OpenAPI 3.x spec parsing: JSON/YAML ingestion, local `$ref` inlining and
//! extraction of a flat, UI-friendly operation list.

use std::collections::HashSet;

use serde_json::Value;

use crate::model::Method;

/// Errors raised while parsing an OpenAPI document.
#[derive(Debug, thiserror::Error)]
pub enum OpenApiError {
    /// The document is neither valid JSON nor valid YAML.
    #[error("failed to parse OpenAPI document: {0}")]
    Parse(String),
    /// The document parsed, but isn't an OpenAPI 3.x document we understand.
    #[error("unsupported OpenAPI document: {0}")]
    Unsupported(String),
}

/// A parsed OpenAPI document, flattened into a form convenient for import UIs.
#[derive(Debug, Clone)]
pub struct ParsedSpec {
    pub title: String,
    pub version: String,
    pub servers: Vec<String>,
    pub operations: Vec<SpecOperation>,
    /// The full document, as parsed JSON — the source of truth for schemas.
    pub raw: Value,
}

/// One `path` + `method` operation, with parameters and schemas already
/// resolved (local `$ref`s inlined).
#[derive(Debug, Clone)]
pub struct SpecOperation {
    /// `operationId`, or a generated `<method>-<path-slug>` fallback. Unique
    /// within the [`ParsedSpec`] it came from.
    pub id: String,
    pub method: Method,
    /// Raw OpenAPI path template, e.g. `/pets/{petId}`.
    pub path: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// Names of `in: path` parameters.
    pub path_params: Vec<String>,
    /// `in: query` parameters as `(name, required)`.
    pub query_params: Vec<(String, bool)>,
    /// `in: header` parameters as `(name, required)`.
    pub header_params: Vec<(String, bool)>,
    pub request_content_type: Option<String>,
    pub request_schema: Option<Value>,
    pub request_example: Option<Value>,
    pub responses: Vec<SpecResponse>,
}

/// One entry of an operation's `responses` map.
#[derive(Debug, Clone)]
pub struct SpecResponse {
    /// `"200"`, `"4XX"` or `"default"`.
    pub status: String,
    pub content_type: Option<String>,
    pub schema: Option<Value>,
}

const HTTP_METHODS: &[(&str, Method)] = &[
    ("get", Method::Get),
    ("put", Method::Put),
    ("post", Method::Post),
    ("delete", Method::Delete),
    ("options", Method::Options),
    ("head", Method::Head),
    ("patch", Method::Patch),
    ("trace", Method::Trace),
];

/// Parse an OpenAPI 3.x document from either JSON or YAML source text.
pub fn parse_spec(text: &str) -> Result<ParsedSpec, OpenApiError> {
    let raw: Value = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(json_err) => serde_yaml_ng::from_str::<Value>(text).map_err(|yaml_err| {
            OpenApiError::Parse(format!("not valid JSON ({json_err}) or YAML ({yaml_err})"))
        })?,
    };

    let Value::Object(_) = &raw else {
        return Err(OpenApiError::Parse(
            "document root must be an object".into(),
        ));
    };

    let openapi_version = raw
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenApiError::Unsupported(
                "missing 'openapi' field (Swagger 2.0 and other formats are not supported)".into(),
            )
        })?
        .to_string();

    if !openapi_version.starts_with("3.") {
        return Err(OpenApiError::Unsupported(format!(
            "unsupported OpenAPI version {openapi_version} (only 3.x is supported)"
        )));
    }
    let is_31 = openapi_version.starts_with("3.1");

    if !is_31 {
        // Best-effort typed parse for convenience/validation; 3.1 documents
        // use schema features (e.g. `type` arrays) the 3.0-shaped
        // `openapiv3` crate doesn't model, so skip it and stay in raw mode.
        let _typed: Result<openapiv3::OpenAPI, _> = serde_json::from_value(raw.clone());
    }

    let title = raw
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let version = raw
        .pointer("/info/version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut servers: Vec<String> = raw
        .get("servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.get("url").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if servers.is_empty() {
        servers.push("/".to_string());
    }

    let operations = extract_operations(&raw);

    Ok(ParsedSpec {
        title,
        version,
        servers,
        operations,
        raw,
    })
}

fn extract_operations(raw: &Value) -> Vec<SpecOperation> {
    let Some(paths) = raw.get("paths").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut used_ids: HashSet<String> = HashSet::new();
    let mut operations = Vec::new();

    for (path, path_item_raw) in paths {
        let path_item = resolve_refs(raw, path_item_raw, 0);
        let Some(path_item_obj) = path_item.as_object() else {
            continue;
        };

        let path_level_params: Vec<Value> = path_item_obj
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for (method_name, method) in HTTP_METHODS {
            let Some(op_raw) = path_item_obj.get(*method_name) else {
                continue;
            };
            let Some(op) = op_raw.as_object() else {
                continue;
            };

            let op_level_params: Vec<Value> = op
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let merged_params = merge_params(&path_level_params, &op_level_params);

            let mut path_params = Vec::new();
            let mut query_params = Vec::new();
            let mut header_params = Vec::new();
            for p in &merged_params {
                let Some(name) = p.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
                match p.get("in").and_then(Value::as_str) {
                    Some("path") => path_params.push(name.to_string()),
                    Some("query") => query_params.push((name.to_string(), required)),
                    Some("header") => header_params.push((name.to_string(), required)),
                    _ => {}
                }
            }

            let summary = op
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tags = op
                .get("tags")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let (request_content_type, request_schema, request_example) = extract_request_body(op);

            let responses = extract_responses(op);

            let id = unique_id(op, method_name, path, &mut used_ids);

            operations.push(SpecOperation {
                id,
                method: *method,
                path: path.clone(),
                summary,
                tags,
                path_params,
                query_params,
                header_params,
                request_content_type,
                request_schema,
                request_example,
                responses,
            });
        }
    }

    operations
}

fn merge_params(path_level: &[Value], op_level: &[Value]) -> Vec<Value> {
    let key_of = |p: &Value| {
        (
            p.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            p.get("in")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    };
    let op_keys: HashSet<(String, String)> = op_level.iter().map(key_of).collect();
    let mut merged: Vec<Value> = path_level
        .iter()
        .filter(|p| !op_keys.contains(&key_of(p)))
        .cloned()
        .collect();
    merged.extend(op_level.iter().cloned());
    merged
}

/// Pick a "preferred" content type key out of a `content` map: JSON first,
/// otherwise whatever is first in document order.
fn pick_content_type(content: &serde_json::Map<String, Value>) -> Option<String> {
    if content.contains_key("application/json") {
        return Some("application/json".to_string());
    }
    content.keys().next().cloned()
}

fn extract_request_body(
    op: &serde_json::Map<String, Value>,
) -> (Option<String>, Option<Value>, Option<Value>) {
    let Some(rb) = op.get("requestBody") else {
        return (None, None, None);
    };
    let Some(content) = rb.get("content").and_then(Value::as_object) else {
        return (None, None, None);
    };
    let Some(ct) = pick_content_type(content) else {
        return (None, None, None);
    };
    let media = content.get(&ct);
    let schema = media.and_then(|m| m.get("schema")).cloned();
    let example = media
        .and_then(|m| m.get("example"))
        .cloned()
        .or_else(|| {
            media
                .and_then(|m| m.get("examples"))
                .and_then(Value::as_object)
                .and_then(|m| m.values().next())
                .and_then(|first| first.get("value"))
                .cloned()
        })
        .or_else(|| schema.as_ref().and_then(|s| s.get("example")).cloned());
    (Some(ct), schema, example)
}

fn extract_responses(op: &serde_json::Map<String, Value>) -> Vec<SpecResponse> {
    let Some(responses) = op.get("responses").and_then(Value::as_object) else {
        return Vec::new();
    };
    responses
        .iter()
        .map(|(status, resp)| {
            let content = resp.get("content").and_then(Value::as_object);
            let (content_type, schema) =
                match content.and_then(|c| pick_content_type(c).map(|ct| (c, ct))) {
                    Some((c, ct)) => {
                        let schema = c.get(&ct).and_then(|m| m.get("schema")).cloned();
                        (Some(ct), schema)
                    }
                    None => (None, None),
                };
            SpecResponse {
                status: status.clone(),
                content_type,
                schema,
            }
        })
        .collect()
}

fn path_slug(path: &str) -> String {
    let mut s = String::new();
    for ch in path.chars() {
        if ch == '{' || ch == '}' {
            continue;
        }
        if ch.is_alphanumeric() {
            s.push(ch);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    s.trim_matches('-').to_string()
}

fn unique_id(
    op: &serde_json::Map<String, Value>,
    method_name: &str,
    path: &str,
    used_ids: &mut HashSet<String>,
) -> String {
    let base = op
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{method_name}-{}", path_slug(path)));

    let id = if used_ids.contains(&base) {
        let mut n = 2;
        loop {
            let candidate = format!("{base}-{n}");
            if !used_ids.contains(&candidate) {
                break candidate;
            }
            n += 1;
        }
    } else {
        base
    };
    used_ids.insert(id.clone());
    id
}

/// Resolve a local JSON Pointer (`#/a/b/0`) against `root`.
fn resolve_pointer<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    let path = pointer.strip_prefix('#')?;
    if path.is_empty() {
        return Some(root);
    }
    let path = path.strip_prefix('/')?;
    let mut cur = root;
    for raw_part in path.split('/') {
        let part = raw_part.replace("~1", "/").replace("~0", "~");
        cur = match cur {
            Value::Object(map) => map.get(&part)?,
            Value::Array(arr) => arr.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Recursively inline local (`#/...`) `$ref`s found anywhere under `node`.
///
/// `depth` counts the number of `$ref` hops followed so far (not tree
/// depth); chains longer than 16 hops (including cycles) are cut short by
/// replacing the unresolved reference with an empty object. External refs
/// (not starting with `#`) are left untouched.
pub fn resolve_refs(root: &Value, node: &Value, depth: u32) -> Value {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                if !r.starts_with('#') {
                    return node.clone();
                }
                if depth >= 16 {
                    return Value::Object(serde_json::Map::new());
                }
                return match resolve_pointer(root, r) {
                    Some(target) => resolve_refs(root, target, depth + 1),
                    None => Value::Object(serde_json::Map::new()),
                };
            }
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_refs(root, v, depth));
            }
            Value::Object(out)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| resolve_refs(root, v, depth)).collect())
        }
        other => other.clone(),
    }
}

/// Extract the path component of a request URL for matching against spec
/// path templates: strips a leading `{{variable}}` or `${variable}` base URL,
/// then `scheme://host`, then the query string.
pub fn url_to_path(url: &str) -> String {
    let mut s = url.trim();
    // Strip a leading {{var}} (and any adjoined scheme it expands to).
    if let Some(rest) = s.strip_prefix("{{") {
        if let Some(end) = rest.find("}}") {
            s = &rest[end + 2..];
        }
    } else if let Some(rest) = s.strip_prefix("${") {
        if let Some(end) = rest.find('}') {
            s = &rest[end + 1..];
        }
    }
    // Strip scheme://host.
    if let Some(p) = s.find("://") {
        let after = &s[p + 3..];
        s = match after.find('/') {
            Some(i) => &after[i..],
            None => "/",
        };
    }
    let s = s.split('?').next().unwrap_or(s);
    if s.is_empty() {
        "/".to_string()
    } else if s.starts_with('/') {
        s.to_string()
    } else {
        format!("/{s}")
    }
}

/// Does a concrete request path match an OpenAPI path template?
/// Template segments in `{braces}` match any single segment.
pub fn path_matches_template(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let p: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    t.len() == p.len()
        && t.iter().zip(&p).all(|(ts, ps)| {
            (ts.starts_with('{') && ts.ends_with('}'))
                || ts == ps
                || ps.contains("{{")
                || ps.contains("${")
        })
}

impl ParsedSpec {
    /// Find the operation matching `method` + a request `url` (which may
    /// contain a `{{baseUrl}}` prefix and `{{variables}}` in segments).
    pub fn find_operation(&self, method: Method, url: &str) -> Option<&SpecOperation> {
        let path = url_to_path(url);
        self.operations
            .iter()
            .find(|op| op.method == method && path_matches_template(&op.path, &path))
    }

    /// Does any operation (any method) match this URL's path?
    pub fn any_path_matches(&self, url: &str) -> bool {
        let path = url_to_path(url);
        self.operations
            .iter()
            .any(|op| path_matches_template(&op.path, &path))
    }

    /// Operations whose path or summary contains `query`
    /// (case-insensitive); everything when `query` is empty.
    pub fn suggest(&self, query: &str) -> Vec<&SpecOperation> {
        let q = url_to_path(query).to_lowercase();
        let q = q.trim_start_matches('/');
        self.operations
            .iter()
            .filter(|op| {
                q.is_empty()
                    || op.path.to_lowercase().contains(q)
                    || op.summary.to_lowercase().contains(q)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn petstore_json() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/openapi/petstore.json"),
        )
        .expect("fixture")
    }

    fn petstore_yaml() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/openapi/petstore.yaml"),
        )
        .expect("fixture")
    }

    #[test]
    fn parses_json_petstore() {
        let spec = parse_spec(&petstore_json()).expect("parse");
        assert_eq!(spec.title, "Petstore");
        assert!(spec
            .servers
            .contains(&"https://api.example.com/v1".to_string()));
        assert!(!spec.operations.is_empty());
    }

    #[test]
    fn url_to_path_strips_var_scheme_host_query() {
        assert_eq!(url_to_path("{{baseUrl}}/pets/1?x=2"), "/pets/1");
        assert_eq!(url_to_path("${env.baseUrl}/pets/1?x=2"), "/pets/1");
        assert_eq!(url_to_path("https://api.example.com/v1/pets"), "/v1/pets");
        assert_eq!(url_to_path("https://api.example.com"), "/");
        assert_eq!(url_to_path("/pets"), "/pets");
        assert_eq!(url_to_path("pets"), "/pets");
    }

    #[test]
    fn template_matching_handles_braces_and_vars() {
        assert!(path_matches_template("/pets/{petId}", "/pets/42"));
        assert!(path_matches_template("/pets", "/pets"));
        assert!(!path_matches_template("/pets", "/pets/42"));
        // A {{variable}} segment in the request URL matches any template seg.
        assert!(path_matches_template("/pets/literal", "/pets/{{id}}"));
        assert!(path_matches_template(
            "/pets/literal",
            "/pets/${bindings.id}"
        ));
    }

    #[test]
    fn find_operation_and_suggest_work_on_petstore() {
        let spec = parse_spec(&petstore_json()).expect("parse");
        let get = spec
            .operations
            .iter()
            .find(|o| o.method == Method::Get)
            .expect("a GET op");
        let url = format!(
            "{{{{baseUrl}}}}{}",
            get.path.replace('{', "{{").replace('}', "}}")
        );
        assert!(spec.find_operation(get.method, &url).is_some(), "{url}");
        assert!(spec.any_path_matches(&url));
        assert!(!spec.suggest("").is_empty());
        assert!(spec
            .find_operation(get.method, "{{baseUrl}}/definitely/not/there")
            .is_none());
    }

    #[test]
    fn parses_yaml_petstore() {
        let spec = parse_spec(&petstore_yaml()).expect("parse");
        assert_eq!(spec.title, "Petstore");
        assert!(!spec.operations.is_empty());
    }

    #[test]
    fn resolves_component_refs() {
        let spec = parse_spec(&petstore_json()).expect("parse");
        let get_pet = spec
            .operations
            .iter()
            .find(|o| o.id == "getPetById")
            .expect("getPetById operation");
        let schema = get_pet
            .responses
            .iter()
            .find(|r| r.status == "200")
            .unwrap();
        let schema = schema.schema.as_ref().expect("schema");
        // The $ref to #/components/schemas/Pet must be inlined.
        assert!(schema.get("$ref").is_none());
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        assert!(schema.get("properties").unwrap().get("id").is_some());
    }

    #[test]
    fn extracts_params() {
        let spec = parse_spec(&petstore_json()).expect("parse");
        let get_pet = spec
            .operations
            .iter()
            .find(|o| o.id == "getPetById")
            .unwrap();
        assert_eq!(get_pet.path_params, vec!["petId".to_string()]);

        let list_pets = spec.operations.iter().find(|o| o.id == "listPets").unwrap();
        assert!(list_pets
            .query_params
            .iter()
            .any(|(n, req)| n == "limit" && !req));
        assert!(list_pets
            .query_params
            .iter()
            .any(|(n, req)| n == "tag" && !req));
    }

    #[test]
    fn fallback_operation_id_is_unique_and_slugged() {
        let spec_text = r#"{
            "openapi": "3.0.3",
            "info": {"title": "t", "version": "1"},
            "paths": {
                "/widgets/{id}": {
                    "get": {"responses": {"200": {"description": "ok"}}},
                    "post": {"responses": {"200": {"description": "ok"}}}
                }
            }
        }"#;
        let spec = parse_spec(spec_text).unwrap();
        let ids: Vec<&str> = spec.operations.iter().map(|o| o.id.as_str()).collect();
        assert!(ids.contains(&"get-widgets-id"));
        assert!(ids.contains(&"post-widgets-id"));
    }

    #[test]
    fn duplicate_ids_get_disambiguated() {
        let spec_text = r#"{
            "openapi": "3.0.3",
            "info": {"title": "t", "version": "1"},
            "paths": {
                "/a": {"get": {"operationId": "dup", "responses": {"200": {"description": "ok"}}}},
                "/b": {"get": {"operationId": "dup", "responses": {"200": {"description": "ok"}}}}
            }
        }"#;
        let spec = parse_spec(spec_text).unwrap();
        let ids: Vec<&str> = spec.operations.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, vec!["dup", "dup-2"]);
    }

    #[test]
    fn rejects_swagger2() {
        let text = r#"{"swagger": "2.0", "info": {"title": "x", "version": "1"}, "paths": {}}"#;
        let err = parse_spec(text).unwrap_err();
        assert!(matches!(err, OpenApiError::Unsupported(_)));
    }

    #[test]
    fn rejects_garbage() {
        let err = parse_spec("not: [valid: yaml: or: json").unwrap_err();
        assert!(matches!(err, OpenApiError::Parse(_)));
    }

    #[test]
    fn parses_openapi_31_in_raw_mode() {
        let text = r#"{
            "openapi": "3.1.0",
            "info": {"title": "T31", "version": "1"},
            "paths": {
                "/x": {
                    "get": {
                        "operationId": "getX",
                        "responses": {
                            "200": {
                                "description": "ok",
                                "content": {
                                    "application/json": {
                                        "schema": {"type": ["string", "null"]}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#;
        let spec = parse_spec(text).expect("3.1 raw parse");
        assert_eq!(spec.title, "T31");
        let op = &spec.operations[0];
        let schema = op.responses[0].schema.as_ref().unwrap();
        assert_eq!(
            schema.get("type").unwrap(),
            &serde_json::json!(["string", "null"])
        );
    }

    #[test]
    fn resolve_refs_caps_cycles() {
        let root = serde_json::json!({
            "components": {
                "schemas": {
                    "A": {"$ref": "#/components/schemas/B"},
                    "B": {"$ref": "#/components/schemas/A"}
                }
            }
        });
        let node = root.pointer("/components/schemas/A").unwrap();
        // Must terminate rather than infinitely recurse / stack overflow.
        let resolved = resolve_refs(&root, node, 0);
        assert!(resolved.is_object());
    }

    #[test]
    fn resolve_refs_leaves_external_refs() {
        let root = serde_json::json!({});
        let node = serde_json::json!({"$ref": "other.yaml#/Foo"});
        let resolved = resolve_refs(&root, &node, 0);
        assert_eq!(resolved, node);
    }
}
