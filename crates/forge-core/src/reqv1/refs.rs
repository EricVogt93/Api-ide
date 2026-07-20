//! Reference addressing: parse a ref string into an [`AssetDescriptor`] and
//! resolve it to an absolute, project-contained file path (or a builtin id).
//! See `docs/architecture/request-format-v1.md` §11.
//!
//! A ref is `alias-or-path[#json-pointer][@version]`, forward slashes only.
//! Aliases come from `project.json`: a file target is an *exact* alias, a
//! directory target a *prefix* alias. Exact beats prefix; longest prefix
//! wins. All resolved paths must stay inside the project root.

use std::path::{Component, Path, PathBuf};

use super::diag::{Code, Diagnostic, Errors};
use super::model::ProjectConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefScheme {
    /// `builtin:<name>@<version>` — a shipped asset, no filesystem.
    Builtin,
    /// An alias or relative path resolving to a project file.
    File,
}

/// A parsed, located reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDescriptor {
    /// Original ref string.
    pub raw: String,
    pub scheme: RefScheme,
    /// For `File`: absolute path. For `Builtin`: the builtin name.
    pub address: String,
    /// JSON Pointer (RFC 6901), if the ref had a `#...` part.
    pub pointer: Option<String>,
    /// `@N` version suffix, if present.
    pub version: Option<u32>,
}

/// The project root plus its alias table. Built once per run.
pub struct RefResolver {
    root: PathBuf,
    /// Exact aliases: alias -> absolute file path.
    exact: Vec<(String, PathBuf)>,
    /// Prefix aliases: alias -> absolute dir path (sorted longest-first).
    prefix: Vec<(String, PathBuf)>,
}

impl RefResolver {
    /// Build from a project root and its parsed `project.json`. Alias targets
    /// are classified file-vs-directory by their on-disk kind (a trailing `/`
    /// or a non-existent target with no extension is treated as a directory).
    pub fn new(root: &Path, project: &ProjectConfig) -> Result<Self, Errors> {
        let root = canonicalize_lenient(root);
        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        let mut seen = std::collections::BTreeSet::new();

        for (alias, target) in &project.aliases {
            if !seen.insert(alias.clone()) {
                return Err(Errors::one(
                    Code::InvalidAlias,
                    format!("duplicate alias {alias:?} in project.json"),
                ));
            }
            let abs = root.join(normalize_rel(target));
            let abs = canonicalize_lenient(&abs);
            if !abs.starts_with(&root) {
                return Err(Errors::one(
                    Code::PathEscape,
                    format!(
                        "alias {alias:?} points outside the project root: {}",
                        abs.display()
                    ),
                ));
            }
            let is_dir = target.ends_with('/')
                || abs.is_dir()
                || (!abs.exists() && abs.extension().is_none());
            if is_dir {
                prefix.push((alias.clone(), abs));
            } else {
                exact.push((alias.clone(), abs));
            }
        }
        // Longest alias first so the most specific prefix wins.
        prefix.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.len()));

        Ok(Self {
            root,
            exact,
            prefix,
        })
    }

    /// Parse and resolve `raw` (a ref appearing in `base_dir`'s document).
    /// `base_dir` is the directory of the referencing request file, used for
    /// relative paths.
    pub fn resolve(&self, raw: &str, base_dir: &Path) -> Result<AssetDescriptor, Diagnostic> {
        if raw.contains('\\') {
            return Err(Diagnostic::new(
                Code::InvalidAlias,
                format!("ref must use forward slashes, got {raw:?}"),
            )
            .with_ref(raw));
        }

        // Split off the JSON Pointer (first '#').
        let (addr_ver, pointer) = match raw.split_once('#') {
            Some((a, p)) => (a, Some(format!("/{}", p.trim_start_matches('/')))),
            None => (raw, None),
        };

        // Split off a trailing @N from the last path segment only.
        let (addr, version) = split_version(addr_ver);

        // builtin:name — no filesystem.
        if let Some(name) = addr.strip_prefix("builtin:") {
            return Ok(AssetDescriptor {
                raw: raw.to_string(),
                scheme: RefScheme::Builtin,
                address: name.to_string(),
                pointer,
                version,
            });
        }

        let abs = self
            .resolve_address(addr, base_dir)
            .map_err(|d| d.with_ref(raw))?;
        let abs = canonicalize_lenient(&abs);
        if !abs.starts_with(&self.root) {
            return Err(Diagnostic::new(
                Code::PathEscape,
                format!("ref resolves outside the project root: {}", abs.display()),
            )
            .with_ref(raw));
        }

        Ok(AssetDescriptor {
            raw: raw.to_string(),
            scheme: RefScheme::File,
            address: abs.to_string_lossy().into_owned(),
            pointer,
            version,
        })
    }

    /// Alias or relative path → absolute path (not yet containment-checked).
    fn resolve_address(&self, addr: &str, base_dir: &Path) -> Result<PathBuf, Diagnostic> {
        // Exact alias wins over any prefix.
        if let Some((_, path)) = self.exact.iter().find(|(a, _)| a == addr) {
            return Ok(path.clone());
        }
        // Longest matching prefix alias (list is sorted longest-first).
        for (alias, dir) in &self.prefix {
            if let Some(rest) = addr.strip_prefix(alias) {
                let rest = rest.trim_start_matches('/');
                if rest.is_empty() {
                    return Err(Diagnostic::new(
                        Code::InvalidAlias,
                        format!("prefix alias {alias:?} needs a path after it"),
                    ));
                }
                let mut path = dir.join(normalize_rel(rest));
                // Executable assets may omit the extension: try .ts then .js.
                if path.extension().is_none() && !path.exists() {
                    for ext in ["ts", "js"] {
                        let with_ext = path.with_extension(ext);
                        if with_ext.exists() {
                            path = with_ext;
                            break;
                        }
                    }
                }
                return Ok(path);
            }
        }
        // A scheme-looking address (`x:y`) that matched no alias is a bad alias.
        if addr.contains(':') && !addr.contains('/') {
            return Err(Diagnostic::new(
                Code::InvalidAlias,
                format!("unknown alias {addr:?}"),
            ));
        }
        // Otherwise a relative (or absolute) path from the request file.
        let p = Path::new(addr);
        Ok(if p.is_absolute() {
            p.to_path_buf()
        } else {
            base_dir.join(normalize_rel(addr))
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Split a trailing `@N` version off the last path segment. `a/b@2` → (`a/b`, 2).
fn split_version(addr: &str) -> (&str, Option<u32>) {
    let last_seg_start = addr.rfind('/').map(|i| i + 1).unwrap_or(0);
    if let Some(at) = addr[last_seg_start..].rfind('@') {
        let at = last_seg_start + at;
        if let Ok(v) = addr[at + 1..].parse::<u32>() {
            return (&addr[..at], Some(v));
        }
    }
    (addr, None)
}

/// Normalize a relative ref into a `PathBuf`, converting `/` to the OS
/// separator and dropping `.` segments. `..` is kept (containment is checked
/// after canonicalization).
fn normalize_rel(rel: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            other => out.push(other),
        }
    }
    out
}

/// Best-effort canonicalization: if the path exists, canonicalize it
/// (resolving symlinks); otherwise fold `.`/`..` lexically. Either way the
/// result is absolute-ish and comparable against the root.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    // Lexical fold for not-yet-existing paths (e.g. an asset that moved).
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectConfig {
        serde_json::from_str(
            r#"{"formatVersion":1,"aliases":{
                "data:users":"./assets/data/users.json",
                "project:assertions":"./assets/assertions"
            }}"#,
        )
        .unwrap()
    }

    fn resolver(tmp: &Path) -> RefResolver {
        std::fs::create_dir_all(tmp.join("assets/data")).unwrap();
        std::fs::create_dir_all(tmp.join("assets/assertions")).unwrap();
        std::fs::write(tmp.join("assets/data/users.json"), "{}").unwrap();
        std::fs::write(tmp.join("assets/assertions/user-created.ts"), "").unwrap();
        RefResolver::new(tmp, &project()).unwrap()
    }

    #[test]
    fn parses_builtin_with_version() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let d = r.resolve("builtin:uuid@1", dir.path()).unwrap();
        assert_eq!(d.scheme, RefScheme::Builtin);
        assert_eq!(d.address, "uuid");
        assert_eq!(d.version, Some(1));
    }

    #[test]
    fn exact_alias_with_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let d = r.resolve("data:users#/valid/alice", dir.path()).unwrap();
        assert_eq!(d.scheme, RefScheme::File);
        assert!(d.address.ends_with("users.json"));
        assert_eq!(d.pointer.as_deref(), Some("/valid/alice"));
    }

    #[test]
    fn prefix_alias_infers_ts_extension_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let d = r
            .resolve("project:assertions/user-created@2", dir.path())
            .unwrap();
        assert!(d.address.ends_with("user-created.ts"), "{}", d.address);
        assert_eq!(d.version, Some(2));
    }

    #[test]
    fn unknown_alias_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let err = r.resolve("data:nope#/x", dir.path()).unwrap_err();
        assert_eq!(err.code, Code::InvalidAlias.as_str());
    }

    #[test]
    fn backslash_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let err = r
            .resolve("assets\\data\\users.json", dir.path())
            .unwrap_err();
        assert_eq!(err.code, Code::InvalidAlias.as_str());
    }

    #[test]
    fn path_escape_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let err = r.resolve("../../../etc/passwd", dir.path()).unwrap_err();
        assert_eq!(err.code, Code::PathEscape.as_str());
    }

    #[test]
    fn relative_path_resolves_from_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolver(dir.path());
        let base = dir.path().join("requests/users");
        std::fs::create_dir_all(&base).unwrap();
        let d = r.resolve("../../assets/data/users.json#/x", &base).unwrap();
        assert!(d.address.ends_with("users.json"));
    }
}
