/// Result of evaluating a single assertion against a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionOutcome {
    /// Human-readable description of the check (from `Check::summary()`).
    pub summary: String,
    pub passed: bool,
    /// Failure detail (expected vs actual) when `passed` is false;
    /// may carry extra info on success too.
    pub message: Option<String>,
}

impl AssertionOutcome {
    pub fn pass(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            passed: true,
            message: None,
        }
    }

    pub fn fail(summary: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            passed: false,
            message: Some(message.into()),
        }
    }
}
