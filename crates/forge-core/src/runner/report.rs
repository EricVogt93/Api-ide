//! JUnit XML report generation from run outcomes.

use super::{RequestOutcome, RunSummary};

/// Render a JUnit XML document (one `<testsuite>`, one `<testcase>` per
/// request execution, assertion failures as `<failure>` entries, transport
/// / script failures as `<error>` entries).
pub fn junit_xml(suite_name: &str, outcomes: &[RequestOutcome], summary: &RunSummary) -> String {
    let time_secs = summary.duration_ms as f64 / 1000.0;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        summary.total, summary.failed, summary.skipped, time_secs
    ));
    out.push_str(&format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        escape_attr(suite_name),
        summary.total,
        summary.failed,
        summary.skipped,
        time_secs
    ));

    for outcome in outcomes {
        write_testcase(&mut out, suite_name, outcome);
    }

    out.push_str("  </testsuite>\n");
    out.push_str("</testsuites>\n");
    out
}

fn write_testcase(out: &mut String, suite_name: &str, outcome: &RequestOutcome) {
    let case_name = format!("[iter {}] {}", outcome.iteration, outcome.name);
    let time_secs = match &outcome.result {
        Ok(res) => res.timing.total.as_secs_f64(),
        Err(_) => 0.0,
    };

    out.push_str(&format!(
        "    <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\">\n",
        escape_attr(&case_name),
        escape_attr(suite_name),
        time_secs
    ));

    match &outcome.result {
        Err(message) => {
            out.push_str(&format!(
                "      <error message=\"{}\">{}</error>\n",
                escape_attr(message),
                escape_text(message)
            ));
        }
        Ok(_) => {
            if let Some(err) = &outcome.script_error {
                out.push_str(&format!(
                    "      <error message=\"{}\">{}</error>\n",
                    escape_attr(err),
                    escape_text(err)
                ));
            }
            for assertion in &outcome.assertions {
                if assertion.passed {
                    continue;
                }
                let detail = assertion.message.clone().unwrap_or_default();
                out.push_str(&format!(
                    "      <failure message=\"{}\">{}</failure>\n",
                    escape_attr(&assertion.summary),
                    escape_text(&detail)
                ));
            }
        }
    }

    if !outcome.script_log.is_empty() {
        out.push_str("      <system-out>");
        out.push_str(&escape_text(&outcome.script_log.join("\n")));
        out.push_str("</system-out>\n");
    }

    out.push_str("    </testcase>\n");
}

/// Escape text for use inside a double-quoted XML attribute value.
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Escape text for use as XML element content.
fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
