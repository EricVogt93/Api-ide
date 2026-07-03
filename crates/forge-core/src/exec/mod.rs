//! HTTP execution engine: turns a [`ResolvedRequest`] into an
//! [`ExecutionResult`] via reqwest/tokio, with timing, cookies,
//! redirect capture and cancellation.

mod cookies;
mod engine;
mod oauth;
mod types;

pub use cookies::*;
pub use engine::*;
pub use oauth::*;
pub use types::*;
