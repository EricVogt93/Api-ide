//! forge-core: GUI-free domain core of the Forge API-testing IDE.
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
pub mod runner;
pub mod script;
pub mod reqv1;
pub mod store;
pub mod vars;

pub const FORMAT_VERSION: u32 = 1;
