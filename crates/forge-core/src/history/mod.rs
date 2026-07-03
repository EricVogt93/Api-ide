//! Execution history: SQLite-backed log of executed requests with search
//! and response diffing.

mod diff;
mod store;

pub use diff::*;
pub use store::*;
