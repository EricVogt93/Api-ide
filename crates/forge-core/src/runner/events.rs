//! Event stream emitted while a run executes. Both the GUI test-results
//! tool window and the CLI printer consume this.

use crate::assert::AssertionOutcome;
use crate::exec::ExecutionResult;

#[derive(Debug, Clone)]
pub enum RunEvent {
    RunStarted {
        /// Total number of request executions (requests × iterations).
        total: usize,
        iterations: usize,
    },
    IterationStarted {
        iteration: usize,
    },
    RequestStarted {
        /// Workspace-relative id of the request file.
        id: String,
        name: String,
        iteration: usize,
    },
    RequestFinished(Box<RequestOutcome>),
    RunFinished(RunSummary),
}

#[derive(Debug, Clone)]
pub struct RequestOutcome {
    pub id: String,
    pub name: String,
    pub iteration: usize,
    /// `Err` = transport-level failure (connect, timeout, …).
    pub result: Result<ExecutionResult, String>,
    pub assertions: Vec<AssertionOutcome>,
    /// Script output lines (console.log from pre/post scripts).
    pub script_log: Vec<String>,
    /// Script-level failure, independent of transport.
    pub script_error: Option<String>,
    /// Variables extracted from the response.
    pub extracted: Vec<(String, String)>,
}

impl RequestOutcome {
    /// A request passes when transport, scripts and every assertion passed.
    pub fn passed(&self) -> bool {
        self.result.is_ok()
            && self.script_error.is_none()
            && self.assertions.iter().all(|a| a.passed)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: u64,
}
