//! Coverage report dialog: the ticket → OpenAPI → history analysis from
//! `forge_pro::report`, rendered as collapsible per-ticket sections with
//! Markdown/JSON export and "comment to Jira" per ticket. Pro-gated at the
//! menu entry that opens it.

use forge_pro::report::{build_report, render_markdown, CoverageReport, DEFAULT_WINDOW};
use forge_core::reqv1::index::ProjectIndex;

use crate::bridge::Bridge;
use crate::state::{AppState, StatusMessage};

#[derive(Default)]
pub struct ReportState {
    pub open: bool,
    report: Option<CoverageReport>,
    error: Option<String>,
}

/// Build (or rebuild) the report from the current workspace and open the
/// dialog. Synchronous: an index scan plus one indexed history query per
/// test.
pub fn open_dialog(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        state.status = Some(StatusMessage::error("No workspace open"));
        return;
    };
    let Some(history) = state.history_store.as_ref() else {
        state.status = Some(StatusMessage::error("No history store available"));
        return;
    };
    state.dialogs.report.error = None;
    match ProjectIndex::scan(&root) {
        Ok(index) => {
            let spec = forge_core::openapi::discover_spec(&root);
            state.dialogs.report.report = Some(build_report(
                &root,
                &index,
                history,
                spec.as_ref(),
                DEFAULT_WINDOW,
                None,
            ));
            state.dialogs.report.open = true;
        }
        Err(diagnostic) => {
            state.status = Some(StatusMessage::error(format!(
                "cannot index the project: {}",
                diagnostic.message
            )));
        }
    }
}

enum DialogAction {
    None,
    Refresh,
    ExportMarkdown,
    ExportJson,
    CommentToJira { key: String, comment: String },
}

pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    if !state.dialogs.report.open {
        return;
    }
    let mut open = state.dialogs.report.open;
    let mut action = DialogAction::None;
    let pro = state.dialogs.license.pro_features();

    egui::Window::new("Coverage report")
        .id(egui::Id::new("report-dialog"))
        .collapsible(false)
        .resizable(true)
        .min_size([680.0, 420.0])
        .default_size([860.0, 600.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let dialog = &state.dialogs.report;
            let Some(report) = &dialog.report else {
                ui.weak("No report built.");
                return;
            };
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    action = DialogAction::Refresh;
                }
                if ui.button("Export Markdown…").clicked() {
                    action = DialogAction::ExportMarkdown;
                }
                if ui.button("Export JSON…").clicked() {
                    action = DialogAction::ExportJson;
                }
                ui.weak(format!("window: last {} runs per test", report.window));
            });
            if let Some(error) = &dialog.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(4.0);
            if let Some(title) = &report.spec_title {
                ui.strong(format!(
                    "OpenAPI {title}: {}/{} operations covered",
                    report.operations_covered, report.operations_total
                ));
            } else {
                ui.weak("No OpenAPI spec found (openapi.* / swagger.* / specs/).");
            }
            ui.add_space(4.0);
            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt("report-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, section) in report.tickets.iter().enumerate() {
                        let heading = section.ticket.as_deref().unwrap_or("Without ticket");
                        egui::CollapsingHeader::new(format!(
                            "{heading} — {} tests, pass rate {:.0}%{}",
                            section.tests.len(),
                            section.pass_rate * 100.0,
                            if section.flaky_tests > 0 {
                                format!(", ⚠ {} flaky", section.flaky_tests)
                            } else {
                                String::new()
                            }
                        ))
                        .id_salt(("report-section", i))
                        .default_open(section.flaky_tests > 0 || section.pass_rate < 1.0)
                        .show(ui, |ui| {
                            if !section.operations.is_empty() {
                                ui.weak(format!("Covers: {}", section.operations.join(", ")));
                                ui.add_space(4.0);
                            }
                            egui::Grid::new(("report-grid", i))
                                .num_columns(7)
                                .striped(true)
                                .spacing([14.0, 4.0])
                                .show(ui, |ui| {
                                    for header in
                                        ["Test", "Runs", "Pass", "Median", "p95", "Hiccups", "Flaky"]
                                    {
                                        ui.strong(header);
                                    }
                                    ui.end_row();
                                    for test in &section.tests {
                                        ui.label(&test.rel_path)
                                            .on_hover_text(test.operation.as_deref().unwrap_or(""));
                                        ui.label(test.runs.to_string());
                                        ui.label(format!("{:.0}%", test.pass_rate * 100.0));
                                        ui.label(format!("{} ms", test.median_ms));
                                        ui.label(format!("{} ms", test.p95_ms));
                                        ui.label(test.hiccups.to_string());
                                        if test.flaky {
                                            ui.colored_label(
                                                ui.visuals().warn_fg_color,
                                                format!("⚠ {} flips", test.flips),
                                            );
                                        } else {
                                            ui.label("—");
                                        }
                                        ui.end_row();
                                    }
                                });
                            if let Some(key) = section
                                .ticket
                                .as_deref()
                                .and_then(forge_pro::jira::ticket_key)
                            {
                                ui.add_space(4.0);
                                if ui
                                    .add_enabled(
                                        pro,
                                        egui::Button::new(format!("Comment report to {key}…")),
                                    )
                                    .on_hover_text("Prefills a Jira comment with this section")
                                    .clicked()
                                {
                                    let single = CoverageReport {
                                        tickets: vec![section.clone()],
                                        ..report.clone()
                                    };
                                    action = DialogAction::CommentToJira {
                                        key,
                                        comment: render_markdown(&single),
                                    };
                                }
                            }
                        });
                    }
                    if !report.uncovered_operations.is_empty() {
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(format!(
                            "Uncovered operations ({})",
                            report.uncovered_operations.len()
                        ))
                        .id_salt("report-uncovered")
                        .show(ui, |ui| {
                            for operation in &report.uncovered_operations {
                                ui.monospace(operation);
                            }
                        });
                    }
                });
        });
    state.dialogs.report.open = open;

    match action {
        DialogAction::None => {}
        DialogAction::Refresh => open_dialog(state),
        DialogAction::ExportMarkdown => export(state, "md", |report| render_markdown(report)),
        DialogAction::ExportJson => export(state, "json", |report| {
            serde_json::to_string_pretty(report).unwrap_or_default()
        }),
        DialogAction::CommentToJira { key, comment } => {
            state.dialogs.jira.open_ticket_with_comment(key, comment, bridge);
        }
    }
}

fn export(state: &mut AppState, extension: &str, render: impl Fn(&CoverageReport) -> String) {
    let Some(report) = &state.dialogs.report.report else {
        return;
    };
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(format!("coverage-report.{extension}"))
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, render(report)) {
        Ok(()) => {
            state.status = Some(StatusMessage::info(format!(
                "Report written to {}",
                path.display()
            )))
        }
        Err(error) => state.dialogs.report.error = Some(format!("cannot write report: {error}")),
    }
}
