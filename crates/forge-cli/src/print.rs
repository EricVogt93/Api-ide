//! Live, IntelliJ-testrunner-style console output for `forge run`.

use std::io::{IsTerminal, Write};

use tokio::sync::mpsc::UnboundedReceiver;

use forge_core::runner::{RequestOutcome, RunEvent, RunSummary};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Whether ANSI colors should be used: respects `NO_COLOR` and falls back
/// to plain text when stdout isn't a terminal.
pub fn supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Drain `rx`, printing each event as it arrives, and return every
/// `RequestOutcome` seen (for JUnit reporting) together with the final
/// summary.
pub async fn run_printer(
    mut rx: UnboundedReceiver<RunEvent>,
    color: bool,
) -> (Vec<RequestOutcome>, RunSummary) {
    let mut outcomes = Vec::new();
    let mut summary = RunSummary::default();
    let mut iteration_count = 1usize;

    while let Some(event) = rx.recv().await {
        match event {
            RunEvent::RunStarted { total, iterations } => {
                iteration_count = iterations.max(1);
                println!("Running {total} request execution(s) across {iterations} iteration(s)\n");
            }
            RunEvent::IterationStarted { iteration } => {
                if iteration_count > 1 {
                    println!("{}", paint(&format!("Iteration {iteration}"), DIM, color));
                }
            }
            RunEvent::RequestStarted { name, .. } => {
                print!("  {name} ... ");
                let _ = std::io::stdout().flush();
            }
            RunEvent::RequestFinished(outcome) => {
                print_outcome(&outcome, color);
                outcomes.push(*outcome);
            }
            RunEvent::RunFinished(s) => {
                summary = s;
            }
        }
    }

    (outcomes, summary)
}

fn print_outcome(outcome: &RequestOutcome, color: bool) {
    let ok = outcome.passed();
    let ms = match &outcome.result {
        Ok(res) => res.timing.total.as_millis(),
        Err(_) => 0,
    };
    let mark = if ok {
        paint("\u{2713}", GREEN, color)
    } else {
        paint("\u{2717}", RED, color)
    };
    println!("{mark} {ms}ms");

    match &outcome.result {
        Err(message) => {
            println!("      {}", paint(&format!("error: {message}"), RED, color));
        }
        Ok(_) => {
            if let Some(err) = &outcome.script_error {
                println!(
                    "      {}",
                    paint(&format!("script error: {err}"), RED, color)
                );
            }
            for assertion in &outcome.assertions {
                if assertion.passed {
                    continue;
                }
                let detail = assertion.message.clone().unwrap_or_default();
                let line = format!("\u{2717} {}: {detail}", assertion.summary);
                println!("      {}", paint(&line, RED, color));
            }
        }
    }
}

/// Print the final pass/fail/skip tally.
pub fn print_summary(summary: &RunSummary, color: bool) {
    println!();
    println!(
        "{} passed, {} failed, {} skipped ({} total) in {}ms",
        paint(&summary.passed.to_string(), GREEN, color),
        paint(&summary.failed.to_string(), RED, color),
        paint(&summary.skipped.to_string(), YELLOW, color),
        summary.total,
        summary.duration_ms
    );
}
