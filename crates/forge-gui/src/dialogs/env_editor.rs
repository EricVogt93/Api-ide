//! Environment manager ("Manage..." in the right Environment panel + View
//! menu): create/delete environments and edit their variable tables,
//! including the gitignored secrets file for `secret: true` rows.
//!
//! Edits autosave: every row change is written straight to `<name>.env.json`
//! / `<name>.secrets.json` and the workspace is reloaded, the same
//! immediate-persistence model `panels::collections` uses for its CRUD
//! operations (there's no separate "dirty" draft to lose on close).

use egui::{TextEdit, Window};

use forge_core::model::EnvVar;
use forge_core::store::{
    create_environment, save_environment, save_secrets, secrets_path, Workspace,
};

use crate::state::{AppState, StatusMessage};

/// Transient state of the environment manager, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct EnvEditorState {
    open: bool,
    selected: Option<String>,
    new_var_key: String,
    /// Environment name awaiting delete confirmation.
    pending_delete: Option<String>,
    /// Which secret rows currently have their value revealed in plain text.
    revealed: std::collections::HashSet<String>,
}

impl EnvEditorState {
    /// Open the manager, selecting `preferred` if given.
    pub fn open(&mut self, preferred: Option<String>) {
        self.open = true;
        self.selected = preferred;
    }
}

/// Render the dialog (and its delete-confirmation popup) if open; no-op
/// otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.env_editor.open {
        return;
    }
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        state.dialogs.env_editor.open = false;
        state.status = Some(StatusMessage::error(
            "Open a workspace before managing environments",
        ));
        return;
    };

    let mut window_open = true;
    let mut new_env_clicked = false;
    let mut needs_reload = false;

    Window::new("Environments")
        .id(egui::Id::new("env-editor-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 440.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                render_env_list(ui, state, &mut new_env_clicked);
                ui.separator();
                if render_selected_env(ui, state) {
                    needs_reload = true;
                }
            });
        });

    if new_env_clicked {
        let n = state
            .workspace
            .as_ref()
            .map(|w| w.environments.len())
            .unwrap_or(0);
        let name = format!("Environment {}", n + 1);
        match create_environment(&root, &name) {
            Ok(_) => {
                needs_reload = true;
                state.dialogs.env_editor.selected = Some(name);
            }
            Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
        }
    }

    if needs_reload {
        reload_workspace(state);
    }

    show_delete_confirm(ctx, state);

    if !window_open {
        state.dialogs.env_editor.open = false;
    }
}

/// Left column: the environment list, "+ New Environment" and "Delete".
fn render_env_list(ui: &mut egui::Ui, state: &mut AppState, new_env_clicked: &mut bool) {
    ui.vertical(|ui| {
        ui.set_width(160.0);
        let names: Vec<String> = state
            .workspace
            .as_ref()
            .map(|w| w.environments.iter().map(|e| e.env.name.clone()).collect())
            .unwrap_or_default();
        egui::ScrollArea::vertical()
            .id_salt("env-list-scroll")
            .max_height(300.0)
            .show(ui, |ui| {
                for name in &names {
                    let selected =
                        state.dialogs.env_editor.selected.as_deref() == Some(name.as_str());
                    if ui.selectable_label(selected, name).clicked() {
                        state.dialogs.env_editor.selected = Some(name.clone());
                    }
                }
            });
        ui.add_space(6.0);
        if ui.button("+ New Environment").clicked() {
            *new_env_clicked = true;
        }
        if let Some(sel) = state.dialogs.env_editor.selected.clone() {
            if ui.button("Delete").clicked() {
                state.dialogs.env_editor.pending_delete = Some(sel);
            }
        }
    });
}

/// Right column: the selected environment's variable table. Returns `true`
/// if a variable was added/edited/removed (the caller reloads the workspace
/// afterwards, once, outside any nested closure borrow of `state`).
fn render_selected_env(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.set_min_width(420.0);
        let Some(name) = state.dialogs.env_editor.selected.clone() else {
            ui.weak("Select an environment, or create a new one.");
            return;
        };
        let Some(loaded) = state
            .workspace
            .as_ref()
            .and_then(|w| w.environment(&name))
            .cloned()
        else {
            ui.weak("Environment not found.");
            return;
        };
        let mut env = loaded.env.clone();
        let mut secrets = loaded.secrets.clone();

        egui::ScrollArea::vertical()
            .id_salt("env-vars-scroll")
            .max_height(320.0)
            .show(ui, |ui| {
                egui::Grid::new("env-editor-grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong("Name");
                        ui.strong("Value");
                        ui.strong("Secret");
                        ui.strong("Description");
                        ui.strong("");
                        ui.end_row();

                        let mut remove: Option<String> = None;
                        for (key, var) in env.variables.iter_mut() {
                            ui.monospace(key.as_str());
                            if var.secret {
                                let revealed = state.dialogs.env_editor.revealed.contains(key);
                                let mut value = secrets.get(key).cloned().unwrap_or_default();
                                if ui
                                    .add(TextEdit::singleline(&mut value).password(!revealed))
                                    .changed()
                                {
                                    secrets.insert(key.clone(), value);
                                    changed = true;
                                }
                            } else {
                                let mut value = var.value.clone().unwrap_or_default();
                                if ui.text_edit_singleline(&mut value).changed() {
                                    var.value = Some(value);
                                    changed = true;
                                }
                            }
                            let mut secret = var.secret;
                            if ui.checkbox(&mut secret, "").changed() {
                                if secret {
                                    var.value = None;
                                } else {
                                    secrets.remove(key);
                                }
                                var.secret = secret;
                                changed = true;
                            }
                            if var.secret {
                                let mut revealed = state.dialogs.env_editor.revealed.contains(key);
                                if ui.checkbox(&mut revealed, "show").changed() {
                                    if revealed {
                                        state.dialogs.env_editor.revealed.insert(key.clone());
                                    } else {
                                        state.dialogs.env_editor.revealed.remove(key);
                                    }
                                }
                            } else {
                                ui.label("");
                            }
                            if ui.text_edit_singleline(&mut var.description).changed() {
                                changed = true;
                            }
                            if ui.small_button("\u{2715}").clicked() {
                                remove = Some(key.clone());
                            }
                            ui.end_row();
                        }
                        if let Some(key) = remove {
                            env.variables.remove(&key);
                            secrets.remove(&key);
                            changed = true;
                        }
                    });
            });

        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut state.dialogs.env_editor.new_var_key);
            let key = state.dialogs.env_editor.new_var_key.trim().to_string();
            if ui
                .add_enabled(!key.is_empty(), egui::Button::new("+ Add variable"))
                .clicked()
            {
                env.variables.insert(key, EnvVar::default());
                state.dialogs.env_editor.new_var_key.clear();
                changed = true;
            }
        });

        if changed {
            if let Err(e) = save_environment(&loaded.file, &env) {
                state.status = Some(StatusMessage::error(e.to_string()));
            } else if let Err(e) = save_secrets(&loaded.file, &secrets) {
                state.status = Some(StatusMessage::error(e.to_string()));
            }
        }
    });
    changed
}

fn show_delete_confirm(ctx: &egui::Context, state: &mut AppState) {
    let Some(name) = state.dialogs.env_editor.pending_delete.clone() else {
        return;
    };
    let mut confirmed = false;
    let mut cancelled = false;
    let mut keep_open = true;

    Window::new("Delete Environment")
        .id(egui::Id::new("env-editor-delete-confirm"))
        .collapsible(false)
        .resizable(false)
        .open(&mut keep_open)
        .show(ctx, |ui| {
            ui.label(format!("Delete \"{name}\"? This cannot be undone."));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

    if confirmed {
        let file = state
            .workspace
            .as_ref()
            .and_then(|w| w.environment(&name))
            .map(|e| e.file.clone());
        if let Some(file) = file {
            let _ = std::fs::remove_file(secrets_path(&file));
            match std::fs::remove_file(&file) {
                Ok(()) => {
                    if state.active_env.as_deref() == Some(name.as_str()) {
                        state.active_env = None;
                    }
                    if state.dialogs.env_editor.selected.as_deref() == Some(name.as_str()) {
                        state.dialogs.env_editor.selected = None;
                    }
                    reload_workspace(state);
                }
                Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
            }
        }
    }
    if confirmed || cancelled || !keep_open {
        state.dialogs.env_editor.pending_delete = None;
    }
}

fn reload_workspace(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        return;
    };
    match Workspace::load(&root) {
        Ok(ws) => state.workspace = Some(ws),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}
