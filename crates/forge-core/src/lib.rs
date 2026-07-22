//! forge-core: GUI-free domain core of the ApiWright API-testing IDE.
//!
//! Everything behavioural lives here — the domain model, file-based
//! persistence, variable interpolation, the HTTP execution engine,
//! assertions, scripting, the test runner, OpenAPI import/contract tests,
//! curl/code-snippet conversion and execution history. Both the GUI and
//! the CLI are thin shells over this crate.

pub mod assert;
pub mod convert;
pub mod exec;
pub mod history;
pub mod model;
pub mod openapi;
pub mod protocols;
pub mod reqv1;
pub mod runner;
pub mod script;
pub mod store;
pub mod vars;

pub const FORMAT_VERSION: u32 = 1;

/// Directory names that project tree walks (import, index, migrate) skip:
/// hidden directories plus well-known dependency/build output.
// ponytail: name-based skip list; parse .gitignore (ignore crate) if someone
// needs custom ignored dirs respected.
pub fn is_ignored_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "target"
                | "dist"
                | "build"
                | "vendor"
                | "venv"
                | "coverage"
                | "__pycache__"
                | "tmp"
        )
}
