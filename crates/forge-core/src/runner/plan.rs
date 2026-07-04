//! Run plan types: what to run and how.

use std::path::PathBuf;

/// What to execute. Paths/ids are workspace-relative (see [`crate::store::Workspace::rel_id`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunScope {
    /// A single request, by workspace-relative file id
    /// (e.g. `collections/payments/create-charge.request.json`).
    Request(String),
    /// Every request under a folder directory (workspace-relative path).
    Folder(String),
    /// Every request of a collection (workspace-relative directory path,
    /// e.g. `collections/payments`).
    Collection(String),
    /// Every request of every collection.
    Workspace,
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Environment name to resolve variables against.
    pub environment: Option<String>,
    /// Data-driven iterations source; `None` = single iteration.
    pub data: Option<DataSource>,
    /// Stop at the first failing request.
    pub bail: bool,
    /// Fixed delay between requests in milliseconds.
    pub delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSource {
    /// CSV with a header row; each data row is one iteration, columns become
    /// iteration variables.
    CsvFile(PathBuf),
    /// JSON array of flat objects; each object is one iteration.
    JsonFile(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("scope not found in workspace: {0}")]
    ScopeNotFound(String),
    #[error("environment not found: {0}")]
    EnvironmentNotFound(String),
    #[error("failed to read data file: {0}")]
    Data(String),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}
