//! JUnit XML report generation from run outcomes.

use super::{RequestOutcome, RunSummary};

/// Render a JUnit XML document (one `<testsuite>`, one `<testcase>` per
/// request execution, assertion failures as `<failure>` entries).
pub fn junit_xml(suite_name: &str, outcomes: &[RequestOutcome], summary: &RunSummary) -> String {
    let _ = (suite_name, outcomes, summary);
    todo!("implemented by the runner module (wave B)")
}
