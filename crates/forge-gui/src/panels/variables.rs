//! Active-request variable inspector and workspace-wide template rename.

use std::collections::BTreeMap;

use egui::{RichText, TextEdit};
use forge_core::store::{rename_workspace_variable, request_variable_counts, VariableRenameError};
use forge_core::vars::VarOrigin;

use crate::state::{AppState, StatusMessage};

#[derive(Default)]
pub struct VariablesUiState {
    rename_from: Option<String>,
    rename_to: String,
}

#[derive(Clone)]
struct Usage {
    rel_id: String,
    request_name: String,
    count: usize,
}

/// Render the inspector. Returns a request id when a usage should be opened.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) -> Option<String> {
    let Some(active_idx) = state.active_tab else {
        ui.centered_and_justified(|ui| ui.weak("Open a request to inspect its variables."));
        return None;
    };
    let workspace = state.workspace.as_ref()?;
    let tab = state.tabs.get(active_idx)?;

    let scopes =
        super::request_editor::build_scopes(workspace, &tab.rel_id, state.active_env.as_deref());
    let counts = match request_variable_counts(&tab.def) {
        Ok(counts) => counts,
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            return None;
        }
    };
    let usages = match workspace_usages(state) {
        Ok(usages) => usages,
        Err(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            return None;
        }
    };

    let mut open_request = None;
    let mut start_rename = None;
    ui.horizontal(|ui| {
        ui.strong("Variables");
        ui.weak(format!("in {}", tab.def.name));
    });
    ui.separator();

    if counts.is_empty() {
        ui.centered_and_justified(|ui| ui.weak("This request has no {{variable}} references."));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("variables-list")
            .show(ui, |ui| {
                egui::Grid::new("variables-grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing(egui::vec2(16.0, 8.0))
                    .show(ui, |ui| {
                        for heading in ["Name", "Value", "Origin", "Usages", ""] {
                            ui.strong(heading);
                        }
                        ui.end_row();

                        for name in counts.keys() {
                            let resolved = scopes.lookup(name);
                            ui.label(RichText::new(name).monospace());
                            variable_value(ui, resolved.as_ref());
                            ui.label(
                                resolved
                                    .as_ref()
                                    .map(|value| origin_label(value.origin))
                                    .unwrap_or("Unresolved"),
                            );

                            let rows = usages.get(name).cloned().unwrap_or_default();
                            let total: usize = rows.iter().map(|usage| usage.count).sum();
                            ui.menu_button(total.to_string(), |ui| {
                                for usage in &rows {
                                    if ui
                                        .button(format!("{}  ×{}", usage.request_name, usage.count))
                                        .on_hover_text(&usage.rel_id)
                                        .clicked()
                                    {
                                        open_request = Some(usage.rel_id.clone());
                                        ui.close();
                                    }
                                }
                            });

                            let dynamic = resolved
                                .as_ref()
                                .is_some_and(|value| value.origin == VarOrigin::Dynamic);
                            if ui
                                .add_enabled(!dynamic, egui::Button::new("Rename…").small())
                                .on_hover_text(if dynamic {
                                    "Built-in dynamic variables cannot be renamed"
                                } else {
                                    "Rename definitions and exact {{name}} references"
                                })
                                .clicked()
                            {
                                start_rename = Some(name.clone());
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    if let Some(name) = start_rename {
        state.variables_ui.rename_to = name.clone();
        state.variables_ui.rename_from = Some(name);
    }
    rename_form(ui, state);
    open_request
}

fn variable_value(ui: &mut egui::Ui, resolved: Option<&forge_core::vars::ResolvedVar>) {
    let Some(resolved) = resolved else {
        ui.weak("—");
        return;
    };
    if resolved.secret {
        ui.label("••••••••");
    } else if resolved.origin == VarOrigin::Dynamic {
        ui.weak("(generated per use)");
    } else {
        let display = truncate(&resolved.value, 80);
        ui.label(&display).on_hover_text(&resolved.value);
    }
}

fn origin_label(origin: VarOrigin) -> &'static str {
    match origin {
        VarOrigin::Dynamic => "Dynamic",
        VarOrigin::Iteration => "Iteration",
        VarOrigin::Runtime => "Runtime",
        VarOrigin::Environment => "Environment",
        VarOrigin::Folder => "Folder",
        VarOrigin::Collection => "Collection",
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn workspace_usages(state: &AppState) -> Result<BTreeMap<String, Vec<Usage>>, VariableRenameError> {
    let Some(workspace) = state.workspace.as_ref() else {
        return Ok(BTreeMap::new());
    };
    let mut usages: BTreeMap<String, Vec<Usage>> = BTreeMap::new();
    for request in workspace.all_requests() {
        let rel_id = workspace.rel_id(&request.file);
        let def = state
            .tabs
            .iter()
            .find(|tab| tab.rel_id == rel_id)
            .map(|tab| &tab.def)
            .unwrap_or(&request.def);
        for (name, count) in request_variable_counts(def)? {
            usages.entry(name).or_default().push(Usage {
                rel_id: rel_id.clone(),
                request_name: def.name.clone(),
                count,
            });
        }
    }
    Ok(usages)
}

fn rename_form(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(old) = state.variables_ui.rename_from.clone() else {
        return;
    };
    ui.separator();
    let dirty = state.tabs.iter().any(|tab| tab.dirty);
    let mut apply = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.label(format!("Rename {old} to"));
        ui.add(
            TextEdit::singleline(&mut state.variables_ui.rename_to)
                .desired_width(220.0)
                .hint_text("newName"),
        );
        apply = ui
            .add_enabled(!dirty, egui::Button::new("Apply"))
            .on_hover_text(if dirty {
                "Save all open requests before a workspace refactor"
            } else {
                "Rename across this workspace"
            })
            .clicked();
        cancel = ui.button("Cancel").clicked();
    });
    if dirty {
        ui.weak("Save all open requests before renaming.");
    }
    ui.weak("Script API strings such as vars.get(\"name\") are not rewritten.");
    if cancel {
        state.variables_ui = VariablesUiState::default();
    } else if apply {
        apply_rename(state, &old);
    }
}

fn apply_rename(state: &mut AppState, old: &str) {
    let new = state.variables_ui.rename_to.trim().to_string();
    let Some(workspace) = state.workspace.as_mut() else {
        return;
    };
    match rename_workspace_variable(workspace, old, &new) {
        Ok(changed) => {
            let definitions: BTreeMap<_, _> = workspace
                .all_requests()
                .into_iter()
                .map(|request| (workspace.rel_id(&request.file), request.def.clone()))
                .collect();
            for tab in &mut state.tabs {
                if let Some(def) = definitions.get(&tab.rel_id) {
                    tab.def = def.clone();
                    tab.dirty = false;
                }
            }
            state.variables_ui = VariablesUiState::default();
            state.status = Some(StatusMessage::info(format!(
                "Renamed {old} to {new} in {changed} location(s)"
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncation_is_unicode_safe() {
        assert_eq!(truncate("äöü", 2), "äö…");
        assert_eq!(truncate("short", 10), "short");
    }
}
