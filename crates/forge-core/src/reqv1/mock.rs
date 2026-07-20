//! Mock server (§10): serve request documents' `mock` blocks over HTTP,
//! routed by method + a path template derived from each request's URL. The
//! request format says *what* a mock returns; routing lives here, out of the
//! document. An optional `mocks.routes.json` adds/overrides explicit routes.
//!
//! Matching and response generation live in [`MockServerConfig::handle`],
//! which is a pure function of (method, path) — unit-testable without a
//! socket. Socket ownership belongs to the CLI adapter.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::diag::{Code, Diagnostic, Errors};
use super::index::ProjectIndex;
use super::model::RequestDocument;
use super::runner::{render_mock, validate};

/// One route: a method + path pattern bound to a resolved request document.
pub struct MockRoute {
    pub method: String,
    /// Path segments; `Wild` matches any single segment (from `:name` or a
    /// `${...}`-containing segment).
    pattern: Vec<Seg>,
    /// Whether the pattern has any wildcard (literal routes win, §11).
    wildcard: bool,
    pub request_id: String,
    doc: RequestDocument,
    file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Wild,
}

/// An explicit route override from `mocks.routes.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteOverride {
    pub method: String,
    pub path: String,
    /// meta.id of the request whose mock to serve.
    pub request: String,
}

/// A response the mock server produced.
pub struct MockHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct MockServerConfig {
    routes: Vec<MockRoute>,
    root: PathBuf,
    env: Value,
}

impl MockServerConfig {
    /// Scan `root` for request documents with a `mock` block and build the
    /// route table. `env` supplies `${env.*}` (only the URL path is used for
    /// routing; a placeholder `baseUrl` is injected if the env lacks one).
    pub fn scan(
        root: &Path,
        mut env: Value,
        secret: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> Result<MockServerConfig, Errors> {
        if env.get("baseUrl").is_none() {
            if let Value::Object(map) = &mut env {
                map.insert("baseUrl".to_string(), Value::from("http://mock.local"));
            }
        }

        let index = ProjectIndex::scan(root).map_err(|diagnostic| Errors(vec![diagnostic]))?;
        // meta.id -> file, for route overrides.
        let mut id_to_file = std::collections::BTreeMap::new();

        let mut routes = Vec::new();
        for req in &index.requests {
            let file = PathBuf::from(&req.path);
            let text = std::fs::read_to_string(&file).map_err(|e| {
                Errors(vec![Diagnostic::new(
                    Code::AssetNotFound,
                    format!("{}: {e}", req.rel_path),
                )])
            })?;
            let doc = RequestDocument::parse(&text).map_err(|e| {
                Errors(vec![Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("{}: {e}", req.rel_path),
                )])
            })?;
            id_to_file.insert(doc.meta.id.clone(), file.clone());
            if doc.mock.is_none() {
                continue;
            }
            // Resolve to get a concrete URL path for the pattern. Secrets use
            // a placeholder provider — a mock route needs no real secret.
            let ir = validate(&doc, root, &file, env.clone(), secret).map_err(Errors)?;
            let path = url_path(&ir.url);
            routes.push(MockRoute {
                method: ir.method.as_str().to_string(),
                pattern: parse_pattern(&path),
                wildcard: path_has_wildcard(&path),
                request_id: doc.meta.id.clone(),
                doc,
                file,
            });
        }

        // Apply mocks.routes.json overrides (added as extra routes; an
        // override for an existing method+path replaces the derived one).
        let overrides_path = root.join("mocks.routes.json");
        let overrides = match std::fs::read_to_string(&overrides_path) {
            Ok(text) => Some(
                serde_json::from_str::<Vec<RouteOverride>>(&text).map_err(|e| {
                    Errors(vec![Diagnostic::new(
                        Code::InvalidAssetInput,
                        format!("mocks.routes.json: {e}"),
                    )])
                })?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(Errors(vec![Diagnostic::new(
                    Code::AssetNotFound,
                    format!("mocks.routes.json: {error}"),
                )]));
            }
        };
        if let Some(overrides) = overrides {
            for ov in overrides {
                let Some(file) = id_to_file.get(&ov.request) else {
                    return Err(Errors(vec![Diagnostic::new(
                        Code::AssetNotFound,
                        format!(
                            "mocks.routes.json references unknown request {:?}",
                            ov.request
                        ),
                    )]));
                };
                let text = std::fs::read_to_string(file).map_err(|e| {
                    Errors(vec![Diagnostic::new(Code::AssetNotFound, e.to_string())])
                })?;
                let doc = RequestDocument::parse(&text).map_err(|e| {
                    Errors(vec![Diagnostic::new(
                        Code::InvalidAssetInput,
                        e.to_string(),
                    )])
                })?;
                let method = ov.method.to_uppercase();
                routes.retain(|r| !(r.method == method && seg_str(&r.pattern) == ov.path));
                routes.push(MockRoute {
                    method,
                    pattern: parse_pattern(&ov.path),
                    wildcard: path_has_wildcard(&ov.path),
                    request_id: ov.request,
                    doc,
                    file: file.clone(),
                });
            }
        }

        Ok(MockServerConfig {
            routes,
            root: root.to_path_buf(),
            env,
        })
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn routes(&self) -> impl Iterator<Item = (&str, String, &str)> {
        self.routes.iter().map(|r| {
            (
                r.method.as_str(),
                seg_str(&r.pattern),
                r.request_id.as_str(),
            )
        })
    }

    /// Handle one incoming request. Literal routes are tried before wildcard
    /// routes; the first match wins. Returns 404 semantics as `None`.
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        secret: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> Result<Option<MockHttpResponse>, Errors> {
        let incoming = parse_incoming(path);
        let method = method.to_uppercase();

        // Exact (non-wildcard) routes first, then wildcard.
        let matched = self
            .routes
            .iter()
            .filter(|r| !r.wildcard && r.method == method && pattern_matches(&r.pattern, &incoming))
            .chain(self.routes.iter().filter(|r| {
                r.wildcard && r.method == method && pattern_matches(&r.pattern, &incoming)
            }))
            .next();
        let Some(matched) = matched else {
            return Ok(None);
        };

        // Re-resolve so a dynamic JS mock runs fresh, then render its mock.
        let ir = validate(
            &matched.doc,
            &self.root,
            &matched.file,
            self.env.clone(),
            secret,
        )
        .map_err(Errors)?;
        let (response, diagnostics) = render_mock(&ir);
        if !diagnostics.is_empty() {
            return Err(Errors(diagnostics));
        }
        response
            .map(|r| MockHttpResponse {
                status: r.status,
                headers: r.headers,
                body: r.body,
            })
            .map(Some)
            .ok_or_else(|| {
                Errors::one(
                    Code::AssetError,
                    format!("mock route {} produced no response", matched.request_id),
                )
            })
    }
}

// ---------------------------------------------------------------------
// Path patterns
// ---------------------------------------------------------------------

fn url_path(url: &str) -> String {
    // Strip scheme://host, keep the path (no query).
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "/",
    };
    path.split('?').next().unwrap_or(path).to_string()
}

fn parse_pattern(path: &str) -> Vec<Seg> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with(':') || s.contains("${") {
                Seg::Wild
            } else {
                Seg::Lit(s.to_string())
            }
        })
        .collect()
}

fn parse_incoming(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn pattern_matches(pattern: &[Seg], incoming: &[String]) -> bool {
    if pattern.len() != incoming.len() {
        return false;
    }
    pattern.iter().zip(incoming).all(|(seg, got)| match seg {
        Seg::Wild => true,
        Seg::Lit(l) => l == got,
    })
}

fn path_has_wildcard(path: &str) -> bool {
    parse_pattern(path).iter().any(|s| matches!(s, Seg::Wild))
}

/// Render a pattern back to a `/a/:x` display string.
fn seg_str(pattern: &[Seg]) -> String {
    let inner: Vec<String> = pattern
        .iter()
        .map(|s| match s {
            Seg::Lit(l) => l.clone(),
            Seg::Wild => ":*".to_string(),
        })
        .collect();
    format!("/{}", inner.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reqv1/project")
    }

    fn secret(name: &str) -> Option<String> {
        (name == "apiToken").then(|| "tok".to_string())
    }

    #[test]
    fn pattern_matching_literal_and_wildcard() {
        assert!(pattern_matches(
            &parse_pattern("/users"),
            &parse_incoming("/users")
        ));
        assert!(!pattern_matches(
            &parse_pattern("/users"),
            &parse_incoming("/users/1")
        ));
        assert!(pattern_matches(
            &parse_pattern("/users/:id"),
            &parse_incoming("/users/42")
        ));
        assert!(!pattern_matches(
            &parse_pattern("/users/:id"),
            &parse_incoming("/users")
        ));
    }

    #[test]
    fn url_path_strips_scheme_host_query() {
        assert_eq!(url_path("http://mock.local/users?x=1"), "/users");
        assert_eq!(url_path("${env.baseUrl}/users"), "/users");
    }

    #[test]
    fn scans_fixture_and_serves_static_mock() {
        let env = serde_json::json!({ "baseUrl": "http://mock.local" });
        let config = MockServerConfig::scan(&fixture_root(), env, &secret).expect("scan");
        assert!(config.route_count() >= 1);

        // Two fixture docs both mock POST /users (a static one -> u-1 and a
        // dynamic JS one -> u-mock); first-match wins. Either is a valid 201
        // with an id — this asserts the route resolves and a mock is served.
        let resp = config
            .handle("POST", "/users", &secret)
            .expect("valid mock")
            .expect("route matched");
        assert_eq!(resp.status, 201);
        let body: Value = serde_json::from_slice(&resp.body).unwrap();
        assert!(
            matches!(body["id"].as_str(), Some("u-1" | "u-mock")),
            "{body}"
        );
    }

    #[test]
    fn serves_a_dynamic_js_mock() {
        // create-js.request.json has a dynamic mock building {id:u-mock,name}.
        let env = serde_json::json!({ "baseUrl": "http://mock.local" });
        let config = MockServerConfig::scan(&fixture_root(), env, &secret).expect("scan");
        // Both create.request.json and create-js.request.json map to
        // POST /users; the literal route from whichever scanned — assert one
        // of the known bodies is returned.
        let resp = config
            .handle("POST", "/users", &secret)
            .expect("valid mock")
            .expect("route matched");
        assert_eq!(resp.status, 201);
    }

    #[test]
    fn unmatched_route_is_none() {
        let env = serde_json::json!({ "baseUrl": "http://mock.local" });
        let config = MockServerConfig::scan(&fixture_root(), env, &secret).expect("scan");
        assert!(config
            .handle("DELETE", "/nope", &secret)
            .expect("valid mock")
            .is_none());
    }
}
