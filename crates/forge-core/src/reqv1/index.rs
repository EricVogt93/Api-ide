//! Project index: a rebuildable, read-only view of the asset store for
//! tooling (asset browser, `forge assets`, editor autocomplete). Never the
//! source of truth — always derivable from the filesystem (§11).
//!
//! Answers the three questions a request author actually has:
//! - *What assets exist?* (by kind, with data assets browsable to any JSON
//!   node so every node yields a copyable ref)
//! - *Who uses this asset?* (reverse references, so "can I change it?")
//! - *What is broken?* (refs that no longer resolve, with the request file
//!   and instance path to fix)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::diag::Diagnostic;
use super::model::{Binding, BodySpec, MockDef, ProjectConfig, RequestDocument};
use super::refs::{RefResolver, RefScheme};
use super::runner::load_project;

/// Where an asset hangs in the store, derived from its location/extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Data,
    Hook,
    Assertion,
    Extractor,
    Generator,
    Mock,
    /// Executable outside the conventional directories.
    Executable,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Data => "data",
            AssetKind::Hook => "hooks",
            AssetKind::Assertion => "assertions",
            AssetKind::Extractor => "extractors",
            AssetKind::Generator => "generators",
            AssetKind::Mock => "mocks",
            AssetKind::Executable => "executable",
        }
    }
}

/// One asset file in the store.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssetEntry {
    /// Absolute path.
    pub path: String,
    /// Path relative to the project root (display form, forward slashes).
    pub rel_path: String,
    pub kind: AssetKind,
    /// Exact alias pointing at this file, if any (preferred ref form).
    pub alias: Option<String>,
    /// `alias/<rest>` form via a prefix alias, if any.
    pub prefix_ref: Option<String>,
    /// Requests using this asset (request rel path + instance path).
    pub used_by: Vec<Usage>,
    /// Parsed content for data assets (drives the browsable JSON tree).
    #[serde(skip)]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    pub request: String,
    pub instance_path: String,
}

/// One request document found in the project.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RequestEntry {
    pub path: String,
    pub rel_path: String,
    pub id: String,
    pub name: String,
    /// Refs this request makes (resolved absolute path or builtin name).
    pub refs: Vec<String>,
}

/// A ref that no longer resolves (or a request that no longer parses).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrokenRef {
    pub request: String,
    pub instance_path: String,
    pub reference: String,
    pub message: String,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ProjectIndex {
    pub root: String,
    pub assets: Vec<AssetEntry>,
    pub requests: Vec<RequestEntry>,
    pub environments: Vec<String>,
    pub broken: Vec<BrokenRef>,
}

impl ProjectIndex {
    /// Scan `root`. Never fails hard: unreadable pieces land in `broken`.
    pub fn scan(root: &Path) -> Result<ProjectIndex, Diagnostic> {
        let project = load_project(root)?;
        let resolver = RefResolver::new(root, &project).map_err(|mut e| e.0.remove(0))?;

        let mut index = ProjectIndex {
            root: root.to_string_lossy().into_owned(),
            ..Default::default()
        };

        index.collect_assets(root, &project);
        index.collect_environments(root);
        index.collect_requests(root, &resolver);
        index.assets.sort_by(|a, b| (a.kind, &a.rel_path).cmp(&(b.kind, &b.rel_path)));
        Ok(index)
    }

    /// The ref string to paste into a request in `base_dir`. Prefers the
    /// exact alias, then a prefix-alias form, then a correct relative path.
    pub fn suggest_ref(&self, asset: &AssetEntry, base_dir: &Path) -> String {
        if let Some(alias) = &asset.alias {
            return alias.clone();
        }
        if let Some(p) = &asset.prefix_ref {
            return p.clone();
        }
        relative_path(base_dir, Path::new(&asset.path))
    }

    fn collect_assets(&mut self, root: &Path, project: &ProjectConfig) {
        // Reverse alias maps for suggest_ref.
        let mut exact_alias: BTreeMap<PathBuf, String> = BTreeMap::new();
        let mut prefix_alias: Vec<(PathBuf, String)> = Vec::new();
        for (alias, target) in &project.aliases {
            let abs = normalize(&root.join(target.trim_start_matches("./")));
            if abs.is_dir() || target.ends_with('/') {
                prefix_alias.push((abs, alias.clone()));
            } else {
                exact_alias.insert(abs, alias.clone());
            }
        }

        let assets_dir = root.join("assets");
        let mut files = Vec::new();
        walk_files(&assets_dir, &mut files);
        for path in files {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "json" | "js" | "ts") {
                continue;
            }
            // Sibling schemas describe their data asset; not assets themselves.
            if path.to_string_lossy().ends_with(".schema.json") {
                continue;
            }
            let kind = classify(&path, ext);
            let data = (kind == AssetKind::Data)
                .then(|| std::fs::read_to_string(&path).ok())
                .flatten()
                .and_then(|t| serde_json::from_str(&t).ok());
            let norm = normalize(&path);
            let prefix_ref = prefix_alias.iter().find_map(|(dir, alias)| {
                norm.strip_prefix(dir).ok().map(|rest| {
                    let rest = rest.to_string_lossy().replace('\\', "/");
                    let rest = rest.strip_suffix(".js").unwrap_or(&rest);
                    format!("{alias}/{rest}")
                })
            });
            self.assets.push(AssetEntry {
                rel_path: relative_path(root, &path),
                path: norm.to_string_lossy().into_owned(),
                kind,
                alias: exact_alias.get(&norm).cloned(),
                prefix_ref,
                used_by: Vec::new(),
                data,
            });
        }
    }

    fn collect_environments(&mut self, root: &Path) {
        let dir = root.join("environments");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "json") {
                    if let Some(stem) = p.file_stem() {
                        self.environments.push(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
        self.environments.sort();
    }

    fn collect_requests(&mut self, root: &Path, resolver: &RefResolver) {
        let mut files = Vec::new();
        walk_files(root, &mut files);
        for path in files {
            if !path.to_string_lossy().ends_with(".request.json") {
                continue;
            }
            let rel = relative_path(root, &path);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    self.broken.push(BrokenRef {
                        request: rel,
                        instance_path: String::new(),
                        reference: String::new(),
                        message: format!("unreadable: {e}"),
                    });
                    continue;
                }
            };
            let doc = match RequestDocument::parse(&text) {
                Ok(d) => d,
                Err(e) => {
                    self.broken.push(BrokenRef {
                        request: rel,
                        instance_path: String::new(),
                        reference: String::new(),
                        message: format!("invalid document: {e}"),
                    });
                    continue;
                }
            };

            let base_dir = path.parent().unwrap_or(root);
            let mut resolved_refs = Vec::new();
            for (instance_path, reference) in collect_refs(&doc) {
                match resolver.resolve(&reference, base_dir) {
                    Ok(desc) if desc.scheme == RefScheme::Builtin => {
                        resolved_refs.push(format!("builtin:{}", desc.address));
                    }
                    Ok(desc) => {
                        let exists = Path::new(&desc.address).exists();
                        if exists {
                            if let Some(asset) =
                                self.assets.iter_mut().find(|a| a.path == desc.address)
                            {
                                asset.used_by.push(Usage {
                                    request: rel.clone(),
                                    instance_path: instance_path.clone(),
                                });
                            }
                            resolved_refs.push(desc.address);
                        } else {
                            self.broken.push(BrokenRef {
                                request: rel.clone(),
                                instance_path,
                                reference,
                                message: format!("target does not exist: {}", desc.address),
                            });
                        }
                    }
                    Err(d) => {
                        self.broken.push(BrokenRef {
                            request: rel.clone(),
                            instance_path,
                            reference,
                            message: d.message,
                        });
                    }
                }
            }

            self.requests.push(RequestEntry {
                path: path.to_string_lossy().into_owned(),
                rel_path: rel,
                id: doc.meta.id.clone(),
                name: doc.meta.name.clone(),
                refs: resolved_refs,
            });
        }
        self.requests.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    }
}

/// Every `(instance_path, ref-or-use)` a document makes.
fn collect_refs(doc: &RequestDocument) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut binding = |prefix: &str, name: &str, b: &Binding| match b {
        Binding::Ref(r) => out.push((format!("{prefix}/{name}"), r.reference.clone())),
        Binding::Use(u) => out.push((format!("{prefix}/{name}"), u.uses.clone())),
        Binding::Value(_) => {}
    };
    for (name, b) in &doc.bindings {
        binding("/bindings", name, b);
    }
    for (name, b) in &doc.matrix {
        binding("/matrix", name, b);
    }
    if let Some(BodySpec::Ref(r)) = &doc.request.body {
        out.push(("/request/body/ref".to_string(), r.reference.clone()));
    }
    for (i, e) in doc.pipeline.iter().enumerate() {
        out.push((format!("/pipeline/{i}/use"), e.uses.clone()));
    }
    match &doc.mock {
        Some(MockDef::Static(m)) => {
            if let Some(BodySpec::Ref(r)) = &m.body {
                out.push(("/mock/body/ref".to_string(), r.reference.clone()));
            }
        }
        Some(MockDef::Dynamic(m)) => out.push(("/mock/use".to_string(), m.uses.clone())),
        None => {}
    }
    out
}

fn classify(path: &Path, ext: &str) -> AssetKind {
    if ext == "json" {
        return AssetKind::Data;
    }
    // Executable: kind from the conventional directory name anywhere in the
    // path (assets/hooks/…, assets/deep/assertions/… both count).
    let p = path.to_string_lossy();
    for (needle, kind) in [
        ("/hooks/", AssetKind::Hook),
        ("/assertions/", AssetKind::Assertion),
        ("/extractors/", AssetKind::Extractor),
        ("/generators/", AssetKind::Generator),
        ("/mocks/", AssetKind::Mock),
    ] {
        if p.replace('\\', "/").contains(needle) {
            return kind;
        }
    }
    AssetKind::Executable
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // Skip generated/VCS dirs.
            let name = entry.file_name();
            if name == ".git" || name == ".forge" || name == "node_modules" || name == "target" {
                continue;
            }
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Forward-slash relative path from `base` to `target` (lexical, walks up
/// with `..` as needed).
pub fn relative_path(base: &Path, target: &Path) -> String {
    let base = normalize(base);
    let target = normalize(target);
    let base_comps: Vec<_> = base.components().collect();
    let target_comps: Vec<_> = target.components().collect();
    let common = base_comps
        .iter()
        .zip(target_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..base_comps.len() {
        parts.push("..".to_string());
    }
    for comp in &target_comps[common..] {
        parts.push(comp.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reqv1/project")
    }

    #[test]
    fn scans_assets_by_kind_with_aliases() {
        let index = ProjectIndex::scan(&fixture_root()).expect("scan");

        let users = index
            .assets
            .iter()
            .find(|a| a.rel_path == "assets/data/users.json")
            .expect("users.json indexed");
        assert_eq!(users.kind, AssetKind::Data);
        assert_eq!(users.alias.as_deref(), Some("data:users"));
        assert!(users.data.as_ref().unwrap().get("valid").is_some());

        let hook = index
            .assets
            .iter()
            .find(|a| a.rel_path == "assets/hooks/service-token.js")
            .expect("hook indexed");
        assert_eq!(hook.kind, AssetKind::Hook);
        assert_eq!(hook.prefix_ref.as_deref(), Some("project:hooks/service-token"));
    }

    #[test]
    fn tracks_usage_across_requests() {
        let index = ProjectIndex::scan(&fixture_root()).expect("scan");
        let users = index
            .assets
            .iter()
            .find(|a| a.rel_path == "assets/data/users.json")
            .unwrap();
        // Used by create.request.json and create-js.request.json bindings.
        assert!(users.used_by.len() >= 2, "{:?}", users.used_by);
        assert!(users
            .used_by
            .iter()
            .any(|u| u.request.ends_with("create.request.json") && u.instance_path == "/bindings/user"));
    }

    #[test]
    fn finds_requests_and_environments() {
        let index = ProjectIndex::scan(&fixture_root()).expect("scan");
        assert!(index.requests.iter().any(|r| r.id == "users.create"));
        assert!(index.requests.iter().any(|r| r.id == "users.create.js"));
        assert_eq!(index.environments, vec!["local".to_string()]);
        assert!(index.broken.is_empty(), "{:?}", index.broken);
    }

    #[test]
    fn reports_broken_refs_with_location() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.json"), r#"{"aliases":{}}"#).unwrap();
        std::fs::create_dir_all(dir.path().join("requests")).unwrap();
        std::fs::write(
            dir.path().join("requests/x.request.json"),
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"http://x"},
                "bindings":{"u":{"ref":"data:missing#/x"}}}"#,
        )
        .unwrap();
        let index = ProjectIndex::scan(dir.path()).expect("scan");
        assert_eq!(index.broken.len(), 1);
        assert_eq!(index.broken[0].instance_path, "/bindings/u");
        assert_eq!(index.broken[0].reference, "data:missing#/x");
    }

    #[test]
    fn suggest_ref_prefers_alias_then_relative() {
        let index = ProjectIndex::scan(&fixture_root()).expect("scan");
        let users =
            index.assets.iter().find(|a| a.rel_path == "assets/data/users.json").unwrap();
        let base = fixture_root().join("requests/users");
        assert_eq!(index.suggest_ref(users, &base), "data:users");

        let hook =
            index.assets.iter().find(|a| a.rel_path == "assets/hooks/service-token.js").unwrap();
        assert_eq!(index.suggest_ref(hook, &base), "project:hooks/service-token");
    }

    #[test]
    fn relative_path_walks_up() {
        let root = fixture_root();
        let rel = relative_path(&root.join("requests/users"), &root.join("assets/data/users.json"));
        assert_eq!(rel, "../../assets/data/users.json");
    }
}
