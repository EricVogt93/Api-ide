//! The run loop: sequential execution with variable chaining, scripts,
//! assertions and event streaming.

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::exec::HttpEngine;
use crate::store::Workspace;

use super::{RunError, RunEvent, RunOptions, RunScope, RunSummary};

/// Execute `scope` sequentially, streaming [`RunEvent`]s as they happen.
///
/// Extracted variables (extractors and script `vars.set`) feed the runtime
/// scope of subsequent requests. Returns the final summary (also emitted as
/// [`RunEvent::RunFinished`]). Respects `cancel` between and during requests.
pub async fn run(
    workspace: &Workspace,
    scope: RunScope,
    options: RunOptions,
    engine: &HttpEngine,
    events: UnboundedSender<RunEvent>,
    cancel: CancellationToken,
) -> Result<RunSummary, RunError> {
    let _ = (workspace, scope, options, engine, events, cancel);
    todo!("implemented by the runner module (wave B)")
}
