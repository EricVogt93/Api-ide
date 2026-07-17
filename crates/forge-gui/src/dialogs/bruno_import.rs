//! Import Bruno (File menu): pick a Bruno collection directory (the one
//! holding `bruno.json`), preview requests/environments plus everything
//! that can't be mapped, and write it into the workspace. Reuses the
//! Postman dialog's write path for the collection tree.

use egui::Window;

use forge_core::convert::{import_bruno, BrunoImport};
use forge_core::store::{create_environment, save_environment};

use super::postman_import::{import_collection, reload_workspace};
use crate::state::{AppState, StatusMessage};

/// Transient state of the Bruno-import dialog, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct BrunoImportState {
    open: bool,
    parsed: Option<Result<BrunoImport, String>>,
    name: String,
    import_environments: bool,
}

impl BrunoImportState {
    /// Open a directory picker for a Bruno collection and parse it
    /// immediately.
    pub fn open(&mut self) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else { return };
        self.parsed = Some(import_bruno(&dir).map_err(|e| e.to_string()));
        self.name = match &self.parsed {
            Some(Ok(import)) => import.collection.name.clone(),
            _ => String::new(),
        };
        self.import_environments = true;
        self.open = true;
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.bruno_import.open {
        return;
    }
    let Some(workspace) = state.workspace.clone() else {
        state.dialogs.bruno_import.open = false;
        state.status = Some(StatusMessage::error("Open a workspace before importing"));
        return;
    };

    let mut window_open = true;
    let mut import_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Import Bruno")
        .id(egui::Id::new("bruno-import-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 420.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let dialog = &mut state.dialogs.bruno_import;
            match &dialog.parsed {
                None => {
                    ui.weak("No collection loaded.");
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(import)) => {
                    ui.label(format!(
                        "Collection \"{}\" — {} request(s), {} environment(s)",
                        import.collection.name,
                        import.collection.request_count(),
                        import.environments.len()
                    ));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Import as:");
                        ui.text_edit_singleline(&mut dialog.name);
                    });
                    if !import.environments.is_empty() {
                        ui.checkbox(&mut dialog.import_environments, "Import environments");
                        ui.weak(
                            "Bruno exports never contain secret values — secrets come over \
                             declared but empty.",
                        );
                    }
                    show_skipped(ui, &import.collection.skipped);
                }
            }

            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
                let can_import = matches!(&state.dialogs.bruno_import.parsed, Some(Ok(_)))
                    && !state.dialogs.bruno_import.name.trim().is_empty();
                if ui.add_enabled(can_import, egui::Button::new("Import")).clicked() {
                    import_clicked = true;
                }
            });
        });

    if import_clicked {
        match do_import(&workspace, &mut state.dialogs.bruno_import) {
            Err(e) => state.status = Some(StatusMessage::error(e)),
            Ok(msg) => {
                state.dialogs.bruno_import.open = false;
                reload_workspace(state);
                state.status = Some(StatusMessage::info(msg));
            }
        }
    }
    if cancel_clicked || !window_open {
        state.dialogs.bruno_import.open = false;
    }
}

fn show_skipped(ui: &mut egui::Ui, skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.colored_label(
        ui.visuals().warn_fg_color,
        format!("{} item(s) can't be imported:", skipped.len()),
    );
    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
        for note in skipped {
            ui.weak(note);
        }
    });
}

fn do_import(
    workspace: &forge_core::store::Workspace,
    dialog: &mut BrunoImportState,
) -> Result<String, String> {
    let Some(Ok(import)) = dialog.parsed.as_ref() else { return Err("nothing to import".to_string()) };
    let name = dialog.name.trim();

    import_collection(workspace, &import.collection, name)?;
    let count = import.collection.request_count();

    let mut env_count = 0;
    if dialog.import_environments {
        for (env, _secrets) in &import.environments {
            // Duplicate environment names are skipped rather than failing
            // the whole import — the collection is already on disk.
            match create_environment(&workspace.root, &env.name) {
                Ok(file) => {
                    save_environment(&file, env).map_err(|e| e.to_string())?;
                    env_count += 1;
                }
                Err(e) => {
                    if !matches!(e, forge_core::store::StoreError::AlreadyExists(_)) {
                        return Err(e.to_string());
                    }
                }
            }
        }
    }

    Ok(format!("Imported {count} request(s) and {env_count} environment(s) from Bruno"))
}
