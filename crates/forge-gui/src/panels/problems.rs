//! Problems tool window (IntelliJ's "Problems" view): live diagnostics for
//! everything currently open — unresolved `{{variables}}`, invalid JSON
//! bodies, plus failures from the most recent run.

use forge_core::model::BodyDef;
use forge_core::vars::{spans, VarScopes};

use crate::state::AppState;
use crate::theme::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub severity: Severity,
    /// Workspace-relative request id the problem belongs to, when clickable.
    pub rel_id: Option<String>,
    /// Short location tag ("URL", "Body", "Run", …).
    pub location: &'static str,
    pub message: String,
}

/// Compute the current problem list from open tabs and the last run.
pub fn collect(state: &AppState) -> Vec<Problem> {
    let mut problems = Vec::new();

    // Diagnostics for every open tab, resolved against the active
    // environment (mirrors what would happen on Send).
    if let Some(ws) = &state.workspace {
        let env = state
            .active_env
            .as_deref()
            .and_then(|name| ws.environment(name));
        for tab in &state.tabs {
            let mut scopes = VarScopes::new();
            if let Some(env) = env {
                scopes = scopes.with_environment(&env.env, &env.secrets);
            }

            let mut check_template = |text: &str, location: &'static str| {
                let unresolved: Vec<String> = spans(text, &scopes)
                    .into_iter()
                    .filter(|s| s.resolved.is_none())
                    .map(|s| s.name)
                    .collect();
                if !unresolved.is_empty() {
                    problems.push(Problem {
                        severity: Severity::Warning,
                        rel_id: Some(tab.rel_id.clone()),
                        location,
                        message: format!(
                            "{}: unresolved variable{} {}",
                            tab.def.name,
                            if unresolved.len() > 1 { "s" } else { "" },
                            unresolved.join(", ")
                        ),
                    });
                }
            };

            check_template(&tab.def.url, "URL");
            for h in &tab.def.headers {
                if h.is_active() {
                    check_template(&h.value, "Headers");
                }
            }
            if let Some(text) = tab.def.body.editor_text() {
                check_template(text, "Body");
            }

            if let BodyDef::Json { text } = &tab.def.body {
                if !text.trim().is_empty() {
                    if let Err(e) = serde_json::from_str::<serde_json::Value>(text) {
                        problems.push(Problem {
                            severity: Severity::Error,
                            rel_id: Some(tab.rel_id.clone()),
                            location: "Body",
                            message: format!("{}: invalid JSON body — {e}", tab.def.name),
                        });
                    }
                }
            }
        }
    }

    // Failures from the most recent run (transport errors and failed
    // assertions, straight from the Run tool window's tree model).
    for req in state.run_log.requests() {
        if let Some(err) = &req.error {
            problems.push(Problem {
                severity: Severity::Error,
                rel_id: Some(req.id.clone()),
                location: "Run",
                message: format!("{}: {err}", req.name),
            });
        }
        for a in req.assertions.iter().filter(|a| !a.passed) {
            problems.push(Problem {
                severity: Severity::Error,
                rel_id: Some(req.id.clone()),
                location: "Run",
                message: format!(
                    "{}: {} — {}",
                    req.name,
                    a.summary,
                    a.message.clone().unwrap_or_default()
                ),
            });
        }
    }

    problems.sort_by_key(|p| match p.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    problems
}

/// Render the Problems tool window. Returns a rel_id to open when the user
/// activates a problem row.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) -> Option<String> {
    let theme = state.theme;
    let problems = collect(state);
    let errors = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .count();
    let warnings = problems.len() - errors;
    let mut open: Option<String> = None;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} {errors} errors", icons::ERROR))
                .color(theme.error_color()),
        );
        ui.label(
            egui::RichText::new(format!("{} {warnings} warnings", icons::WARNING))
                .color(theme.warn_color()),
        );
    });
    ui.separator();

    if problems.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("No problems — everything looks good.")
                    .color(theme.dim_color()),
            );
        });
        return None;
    }

    egui::ScrollArea::vertical()
        .id_salt("problems-sa-1")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, problem) in problems.iter().enumerate() {
                let (icon, color) = match problem.severity {
                    Severity::Error => (icons::ERROR, theme.error_color()),
                    Severity::Warning => (icons::WARNING, theme.warn_color()),
                };
                let row = ui
                    .horizontal(|ui| {
                        ui.label(egui::RichText::new(icon).color(color));
                        ui.label(
                            egui::RichText::new(format!("[{}]", problem.location))
                                .color(theme.dim_color())
                                .small(),
                        );
                        ui.label(&problem.message);
                    })
                    .response;
                let row = ui.interact(row.rect, ui.id().with(("problem", i)), egui::Sense::click());
                if row.double_clicked() || row.clicked() {
                    open = problem.rel_id.clone();
                }
            }
        });

    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use forge_core::model::{Method, RequestDef};

    fn state_with_tab(url: &str, body: Option<&str>) -> AppState {
        let mut state = AppState::default();
        let mut def = RequestDef::new("Test Req", Method::Get, url);
        if let Some(b) = body {
            def.body = BodyDef::Json {
                text: b.to_string(),
            };
        }
        state.open_tab("collections/c/test.request.json".to_string(), def);
        // No workspace => tab diagnostics are skipped; give it a fake one is
        // heavy, so tab checks are exercised only when a workspace exists.
        state
    }

    #[test]
    fn no_workspace_means_no_tab_problems() {
        let state = state_with_tab("{{missing}}/x", Some("{ broken"));
        assert!(collect(&state).is_empty());
    }
}
