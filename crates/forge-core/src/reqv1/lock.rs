//! Optional lockfile (`.forge/lock.json`): pins each resolved file asset by
//! a content hash so CI (and shared fixtures) reproduce exactly, and a
//! changed fixture is caught. Rebuildable, never the project definition
//! (§16). Off by default: only `forge lock` writes it and only
//! `run-v1 --frozen` checks it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::diag::{Code, Diagnostic};
use super::index::ProjectIndex;

pub const LOCK_REL_PATH: &str = ".forge/lock.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    pub format_version: u32,
    /// Project-relative asset path → `sha256:<hex>` of its bytes.
    pub assets: BTreeMap<String, String>,
}

impl Lockfile {
    /// Build a lockfile from a scanned project: hash every file asset the
    /// index found.
    pub fn from_index(index: &ProjectIndex) -> Result<Lockfile, Diagnostic> {
        let mut assets = BTreeMap::new();
        for asset in &index.assets {
            let hash = hash_file(&asset.path)?;
            assets.insert(asset.rel_path.clone(), hash);
        }
        Ok(Lockfile {
            format_version: 1,
            assets,
        })
    }

    /// Scan `root` and build a lockfile.
    pub fn build(root: &Path) -> Result<Lockfile, Diagnostic> {
        let index = ProjectIndex::scan(root)?;
        Lockfile::from_index(&index)
    }

    pub fn write(&self, root: &Path) -> Result<(), Diagnostic> {
        let dir = root.join(".forge");
        std::fs::create_dir_all(&dir)
            .map_err(|e| Diagnostic::new(Code::AssetError, format!("cannot create .forge: {e}")))?;
        let text = serde_json::to_string_pretty(self).map_err(|e| {
            Diagnostic::new(Code::AssetError, format!("cannot serialize lockfile: {e}"))
        })?;
        std::fs::write(root.join(LOCK_REL_PATH), text)
            .map_err(|e| Diagnostic::new(Code::AssetError, format!("cannot write lockfile: {e}")))
    }

    pub fn read(root: &Path) -> Result<Lockfile, Diagnostic> {
        let path = root.join(LOCK_REL_PATH);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Diagnostic::new(
                Code::AssetNotFound,
                format!("no lockfile at {LOCK_REL_PATH}: {e}"),
            )
        })?;
        serde_json::from_str(&text)
            .map_err(|e| Diagnostic::new(Code::InvalidAssetInput, format!("invalid lockfile: {e}")))
    }

    /// Verify the current project against this lockfile. Returns a diagnostic
    /// per drift: a changed hash, a missing (deleted) asset, or a new
    /// unlocked asset. Empty result = clean.
    pub fn verify(&self, root: &Path) -> Result<Vec<Diagnostic>, Diagnostic> {
        let current = Lockfile::build(root)?;
        let mut diags = Vec::new();
        for (path, hash) in &self.assets {
            match current.assets.get(path) {
                None => diags.push(Diagnostic::new(
                    Code::AssetNotFound,
                    format!("locked asset {path} is missing"),
                )),
                Some(now) if now != hash => diags.push(Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("asset {path} changed since lock ({hash} -> {now})"),
                )),
                Some(_) => {}
            }
        }
        for path in current.assets.keys() {
            if !self.assets.contains_key(path) {
                diags.push(Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("asset {path} is not in the lockfile (run `forge lock`)"),
                ));
            }
        }
        Ok(diags)
    }
}

fn hash_file(path: &str) -> Result<String, Diagnostic> {
    let bytes = std::fs::read(path)
        .map_err(|e| Diagnostic::new(Code::AssetNotFound, format!("cannot read {path}: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("sha256:{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reqv1/project")
    }

    #[test]
    fn build_hashes_every_asset() {
        let lock = Lockfile::build(&fixture_root()).expect("build");
        assert!(lock.assets.contains_key("assets/data/users.json"));
        assert!(lock.assets.values().all(|h| h.starts_with("sha256:")));
    }

    #[test]
    fn verify_clean_against_itself() {
        let root = fixture_root();
        let lock = Lockfile::build(&root).expect("build");
        let diags = lock.verify(&root).expect("verify");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn verify_detects_a_changed_asset() {
        let root = fixture_root();
        let mut lock = Lockfile::build(&root).expect("build");
        // Corrupt one recorded hash to simulate drift.
        *lock.assets.get_mut("assets/data/users.json").unwrap() = "sha256:deadbeef".to_string();
        let diags = lock.verify(&root).expect("verify");
        assert!(diags
            .iter()
            .any(|d| d.message.contains("users.json") && d.message.contains("changed")));
    }

    #[test]
    fn verify_detects_a_missing_and_a_new_asset() {
        let root = fixture_root();
        let mut lock = Lockfile::build(&root).expect("build");
        lock.assets
            .insert("assets/data/ghost.json".to_string(), "sha256:x".to_string());
        lock.assets.remove("assets/data/tenants.json");
        let diags = lock.verify(&root).expect("verify");
        assert!(diags
            .iter()
            .any(|d| d.message.contains("ghost.json") && d.message.contains("missing")));
        assert!(diags.iter().any(
            |d| d.message.contains("tenants.json") && d.message.contains("not in the lockfile")
        ));
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("project.json"), r#"{"aliases":{}}"#).unwrap();
        std::fs::create_dir_all(dir.path().join("assets/data")).unwrap();
        std::fs::write(dir.path().join("assets/data/x.json"), "{}").unwrap();
        let lock = Lockfile::build(dir.path()).unwrap();
        lock.write(dir.path()).unwrap();
        let read = Lockfile::read(dir.path()).unwrap();
        assert_eq!(read.assets, lock.assets);
    }
}
