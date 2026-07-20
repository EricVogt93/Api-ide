use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::*;

use super::{io_err, load_json, save_json, StoreError, StoreResult};

pub const WORKSPACE_FILE: &str = "forge.json";
pub const COLLECTION_FILE: &str = "collection.json";
pub const FOLDER_FILE: &str = "folder.json";
pub const REQUEST_SUFFIX: &str = ".request.json";
pub const ENV_SUFFIX: &str = ".env.json";
pub const SECRETS_SUFFIX: &str = ".secrets.json";
pub const COLLECTIONS_DIR: &str = "collections";
pub const ENVIRONMENTS_DIR: &str = "environments";
pub const SPECS_DIR: &str = "specs";
pub const LOCAL_DIR: &str = ".forge-local";

/// A fully loaded workspace tree.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub meta: WorkspaceMeta,
    pub environments: Vec<LoadedEnvironment>,
    pub collections: Vec<CollectionNode>,
}

#[derive(Debug, Clone)]
pub struct LoadedEnvironment {
    /// Path of the committed `<name>.env.json` file.
    pub file: PathBuf,
    pub env: Environment,
    /// Values from the sibling gitignored secrets file.
    pub secrets: SecretValues,
}

#[derive(Debug, Clone)]
pub struct CollectionNode {
    pub dir: PathBuf,
    pub meta: CollectionMeta,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone)]
pub enum TreeNode {
    Folder(FolderNode),
    Request(RequestNode),
}

#[derive(Debug, Clone)]
pub struct FolderNode {
    pub dir: PathBuf,
    pub meta: FolderMeta,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone)]
pub struct RequestNode {
    pub file: PathBuf,
    pub def: RequestDef,
}

impl TreeNode {
    pub fn file_name(&self) -> String {
        let p = match self {
            TreeNode::Folder(f) => &f.dir,
            TreeNode::Request(r) => &r.file,
        };
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    pub fn display_name(&self) -> String {
        match self {
            TreeNode::Folder(f) if !f.meta.name.is_empty() => f.meta.name.clone(),
            TreeNode::Folder(f) => f
                .dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            TreeNode::Request(r) => r.def.name.clone(),
        }
    }
}

impl FolderNode {
    /// Depth-first iteration over all requests below this folder.
    pub fn requests(&self) -> Vec<&RequestNode> {
        collect_requests(&self.children)
    }
}

impl CollectionNode {
    pub fn requests(&self) -> Vec<&RequestNode> {
        collect_requests(&self.children)
    }
}

fn collect_requests(children: &[TreeNode]) -> Vec<&RequestNode> {
    let mut out = Vec::new();
    for child in children {
        match child {
            TreeNode::Request(r) => out.push(r),
            TreeNode::Folder(f) => out.extend(collect_requests(&f.children)),
        }
    }
    out
}

impl Workspace {
    /// Load a workspace from `root`. Fails if `forge.json` is missing.
    pub fn load(root: impl Into<PathBuf>) -> StoreResult<Workspace> {
        let root = root.into();
        let meta_path = root.join(WORKSPACE_FILE);
        if !meta_path.is_file() {
            return Err(StoreError::NotAWorkspace(root));
        }
        let meta: WorkspaceMeta = load_json(&meta_path)?;
        check_format(meta.format, &meta_path)?;

        let environments = load_environments(&root.join(ENVIRONMENTS_DIR))?;
        let collections = load_collections(&root.join(COLLECTIONS_DIR))?;

        Ok(Workspace {
            root,
            meta,
            environments,
            collections,
        })
    }

    /// Create a fresh workspace directory skeleton.
    pub fn create(root: impl Into<PathBuf>, name: &str) -> StoreResult<Workspace> {
        let root = root.into();
        let meta_path = root.join(WORKSPACE_FILE);
        if meta_path.exists() {
            return Err(StoreError::AlreadyExists(meta_path));
        }
        for dir in [COLLECTIONS_DIR, ENVIRONMENTS_DIR, SPECS_DIR] {
            let p = root.join(dir);
            std::fs::create_dir_all(&p).map_err(io_err(&p))?;
        }
        let meta = WorkspaceMeta::new(name);
        save_json(&meta_path, &meta)?;
        ensure_gitignore(&root)?;
        Ok(Workspace {
            root,
            meta,
            environments: Vec::new(),
            collections: Vec::new(),
        })
    }

    pub fn save_meta(&self) -> StoreResult<()> {
        save_json(&self.root.join(WORKSPACE_FILE), &self.meta)
    }

    /// Stable runtime id of a node: its path relative to the workspace root.
    pub fn rel_id(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn environment(&self, name: &str) -> Option<&LoadedEnvironment> {
        self.environments.iter().find(|e| e.env.name == name)
    }

    /// All requests of all collections, depth-first.
    pub fn all_requests(&self) -> Vec<&RequestNode> {
        self.collections.iter().flat_map(|c| c.requests()).collect()
    }

    /// Find a request node by its workspace-relative id.
    pub fn find_request(&self, rel_id: &str) -> Option<&RequestNode> {
        let path = self.root.join(rel_id);
        self.all_requests().into_iter().find(|r| r.file == path)
    }
}

fn check_format(found: u32, path: &Path) -> StoreResult<()> {
    if found > crate::FORMAT_VERSION {
        return Err(StoreError::UnsupportedFormat {
            path: path.to_path_buf(),
            found,
            supported: crate::FORMAT_VERSION,
        });
    }
    Ok(())
}

fn load_environments(dir: &Path) -> StoreResult<Vec<LoadedEnvironment>> {
    let mut envs = Vec::new();
    if !dir.is_dir() {
        return Ok(envs);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(ENV_SUFFIX))
        })
        .collect();
    entries.sort();
    for file in entries {
        let env: Environment = load_json(&file)?;
        check_format(env.format, &file)?;
        let secrets = load_secrets(&file)?;
        envs.push(LoadedEnvironment { file, env, secrets });
    }
    Ok(envs)
}

/// Load the sibling secrets file of a `*.env.json` path (missing → empty).
pub fn load_secrets(env_file: &Path) -> StoreResult<SecretValues> {
    let path = secrets_path(env_file);
    if path.is_file() {
        load_json(&path)
    } else {
        Ok(BTreeMap::new())
    }
}

pub fn save_secrets(env_file: &Path, secrets: &SecretValues) -> StoreResult<()> {
    save_json(&secrets_path(env_file), secrets)
}

pub fn secrets_path(env_file: &Path) -> PathBuf {
    let name = env_file.file_name().unwrap_or_default().to_string_lossy();
    let base = name.strip_suffix(ENV_SUFFIX).unwrap_or(&name);
    env_file.with_file_name(format!("{base}{SECRETS_SUFFIX}"))
}

fn load_collections(dir: &Path) -> StoreResult<Vec<CollectionNode>> {
    let mut cols = Vec::new();
    if !dir.is_dir() {
        return Ok(cols);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join(COLLECTION_FILE).is_file())
        .collect();
    entries.sort();
    for col_dir in entries {
        let meta_path = col_dir.join(COLLECTION_FILE);
        let meta: CollectionMeta = load_json(&meta_path)?;
        check_format(meta.format, &meta_path)?;
        let children = load_children(&col_dir, &meta.order)?;
        cols.push(CollectionNode {
            dir: col_dir,
            meta,
            children,
        });
    }
    Ok(cols)
}

fn load_children(dir: &Path, order: &[String]) -> StoreResult<Vec<TreeNode>> {
    let mut nodes: Vec<TreeNode> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io_err(dir))? {
        let path = entry.map_err(io_err(dir))?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if path.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            let meta_path = path.join(FOLDER_FILE);
            let meta: FolderMeta = if meta_path.is_file() {
                load_json(&meta_path)?
            } else {
                FolderMeta::default()
            };
            let children = load_children(&path, &meta.order)?;
            nodes.push(TreeNode::Folder(FolderNode {
                dir: path,
                meta,
                children,
            }));
        } else if name.ends_with(REQUEST_SUFFIX) {
            let def: RequestDef = load_json(&path)?;
            check_format(def.format, &path)?;
            nodes.push(TreeNode::Request(RequestNode { file: path, def }));
        }
    }
    sort_by_order(&mut nodes, order);
    Ok(nodes)
}

/// Order nodes by the `order` array; entries not listed are appended
/// alphabetically (case-insensitive) after the listed ones.
fn sort_by_order(nodes: &mut [TreeNode], order: &[String]) {
    nodes.sort_by(|a, b| {
        let (an, bn) = (a.file_name(), b.file_name());
        let ai = order.iter().position(|o| *o == an);
        let bi = order.iter().position(|o| *o == bn);
        match (ai, bi) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => an.to_lowercase().cmp(&bn.to_lowercase()),
        }
    });
}

/// Make sure the workspace `.gitignore` covers local state and secrets.
pub fn ensure_gitignore(root: &Path) -> StoreResult<()> {
    let path = root.join(".gitignore");
    let required = [format!("{LOCAL_DIR}/"), format!("*{SECRETS_SUFFIX}")];
    let existing = match std::fs::read_to_string(&path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io_err(&path)(error)),
    };
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut changed = false;
    for req in required {
        if !lines.iter().any(|l| l.trim() == req) {
            lines.push(req);
            changed = true;
        }
    }
    if changed {
        std::fs::write(&path, lines.join("\n") + "\n").map_err(io_err(&path))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_gitignore;

    #[test]
    fn invalid_utf8_gitignore_is_not_overwritten() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join(".gitignore");
        let original = [0xff, 0xfe, b'\n'];
        std::fs::write(&path, original).expect("fixture");

        let error = ensure_gitignore(root.path()).expect_err("invalid UTF-8 must fail");

        assert!(error.to_string().contains(".gitignore"));
        assert_eq!(std::fs::read(path).expect("read back"), original);
    }
}
