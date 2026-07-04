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
