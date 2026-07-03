//! HTTP execution engine: turns a [`ResolvedRequest`] into an
//! [`ExecutionResult`] via reqwest/tokio, with timing, cookies,
//! redirect capture and cancellation.

mod types;

pub use types::*;
