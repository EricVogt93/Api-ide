//! File-based, git-friendly persistence.
//!
//! A workspace is a plain directory tree (see the repository README for the
//! layout). Identity is the file/directory name; `order` arrays in
//! collection/folder metadata control tree ordering, with entries missing
//! from the list appended alphabetically so that git merges stay trivial.

mod ops;
mod variables;
mod workspace;

pub use ops::*;
pub use variables::*;
pub use workspace::*;

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: invalid JSON: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0} is not a Forge workspace (missing forge.json)")]
    NotAWorkspace(PathBuf),
    #[error("{path}: unsupported format version {found} (this build supports up to {supported})")]
    UnsupportedFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("{0} already exists")]
    AlreadyExists(PathBuf),
    #[error(
        "invalid name {0:?}: names must not be empty, contain path separators, or start with a dot"
    )]
    InvalidName(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

pub(crate) fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Read + parse a JSON file.
pub fn load_json<T: DeserializeOwned>(path: &Path) -> StoreResult<T> {
    let text = std::fs::read_to_string(path).map_err(io_err(path))?;
    serde_json::from_str(&text).map_err(|source| StoreError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Serialize as pretty-printed 2-space JSON with a trailing newline —
/// the canonical on-disk representation (stable diffs).
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> StoreResult<()> {
    let mut text = serde_json::to_string_pretty(value).map_err(|source| StoreError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
    }
    std::fs::write(path, text).map_err(io_err(path))
}

/// Validate a user-supplied file/folder name.
pub fn validate_name(name: &str) -> StoreResult<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) || name.contains("..")
    {
        return Err(StoreError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Turn a display name into a filesystem-safe slug (`Create Charge` → `create-charge`).
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true;
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}
