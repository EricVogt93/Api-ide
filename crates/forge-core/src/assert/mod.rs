//! Assertion evaluation, assertion generation and value extraction.

mod eval;
mod extract;
mod generate;
mod outcome;
pub mod schema;

pub use eval::*;
pub use extract::*;
pub use generate::*;
pub use outcome::*;
pub use schema::*;

/// Test-only helpers for building [`crate::exec::ExecutionResult`] fixtures,
/// shared by the unit tests in this module's submodules.
#[cfg(test)]
pub(crate) mod test_support {
    use std::time::Duration;

    use chrono::Utc;

    use crate::exec::{ExecutionResult, Sizes, TimingBreakdown};

    /// Build a minimal `ExecutionResult` for tests: given status, headers,
    /// raw body bytes and a total-timing figure (in milliseconds).
    pub fn exec_result(
        status: u16,
        headers: &[(&str, &str)],
        body: &[u8],
        total_ms: u64,
    ) -> ExecutionResult {
        ExecutionResult {
            status,
            status_text: String::new(),
            http_version: "HTTP/1.1".to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
            timing: TimingBreakdown {
                total: Duration::from_millis(total_ms),
                ..Default::default()
            },
            size: Sizes::default(),
            effective_url: "http://example.test/".to_string(),
            redirect_chain: Vec::new(),
            cookies_set: Vec::new(),
            executed_at: Utc::now(),
        }
    }
}
