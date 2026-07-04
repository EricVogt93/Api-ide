//! Test runner: executes a run plan (request / folder / collection / suite)
//! sequentially with variable chaining, data-driven iterations and
//! JUnit XML reporting.

mod events;
mod plan;
mod report;
mod resolve;
mod run;

pub use events::*;
pub use plan::*;
pub use report::*;
pub use resolve::*;
pub use run::*;

// Re-exported so downstream crates (the CLI, the GUI) can build a
// `CancellationToken` to pass into [`run`] without adding their own direct
// dependency on `tokio-util`.
pub use tokio_util::sync::CancellationToken;
