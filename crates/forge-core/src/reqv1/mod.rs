//! Request format v1: a local-first, Git-friendly, deterministic request
//! format and execution engine. One request per JSON file, all reuse by
//! reference into an asset store, all behavior as ordered pipeline assets.
//!
//! See `docs/architecture/request-format-v1.md` for the full specification
//! and `schemas/request-v1.schema.json` for the document schema.
//!
//! Stage flow (§12): parse → refs → bindings → variables → canonical IR →
//! pipeline → HTTP/mock → result. Stages up to the IR are pure (no network)
//! and back [`validate`]; running adds the HTTP send.

pub mod build;
pub mod diag;
pub mod ir;
pub mod model;
pub mod pipeline;
pub mod refs;
pub mod resolve;
pub mod runner;
pub mod vars;

pub use build::{build_ir, BuildInputs};
pub use diag::{Code, Diagnostic, Errors, Severity};
pub use ir::{ResolvedBody, ResolvedHeader, ResolvedRequest};
pub use model::{Binding, ProjectConfig, RequestDocument};
pub use pipeline::{AssertionResult, ResponseView};
pub use refs::{AssetDescriptor, RefResolver, RefScheme};
pub use resolve::DataStore;
pub use runner::{
    load_environment, load_project, run, validate, HttpResultView, RunMode, RunResult, RunStatus,
};
