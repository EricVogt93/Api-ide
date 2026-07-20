//! Import Postman (File menu): pick a Postman collection or environment
//! export (both are .json — the parser detects which one it is), preview
//! what will be imported plus everything that can't be (pm.* scripts,
//! unsupported auth), and write it into the workspace.

use std::path::{Path, PathBuf};

use egui::Window;

use forge_core::convert::{
    parse_postman, parse_postman_environment, ImportedCollection, ImportedItem, PostmanError,
};
use forge_core::model::{Environment, FolderMeta, SecretValues};
use forge_core::store::{
    create_collection, create_environment, create_folder, create_request, save_collection_meta,
    save_environment, save_folder_meta, save_secrets, Workspace,
};

use crate::state::{AppState, StatusMessage};

/// What the picked file turned out to contain.
#[allow(clippy::large_enum_variant)] // one short-lived instance per dialog
enum Parsed {
    Collection(ImportedCollection),
    Environment(Environment, SecretValues),
}

/// Transient state of the Postman-import dialog, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct PostmanImportState {
    open: bool,
    parsed: Option<Result<Parsed, String>>,
    name: String,
}

impl PostmanImportState {
    /// Open a file picker for a Postman JSON export and parse it
    /// immediately. Collection vs environment is auto-detected.
    pub fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Postman export", &["json"])
            .pick_file()
        else {
            return;
        };
        self.parsed = Some(parse_file(&path));
        self.name = match &self.parsed {
            Some(Ok(Parsed::Collection(c))) => c.name.clone(),
            Some(Ok(Parsed::Environment(e, _))) => e.name.clone(),
            _ => String::new(),
        };
        self.open = true;
    }
}

fn parse_file(path: &PathBuf) -> Result<Parsed, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    match parse_postman(&text) {
        Ok(c) => Ok(Parsed::Collection(c)),
        // Only fall through to the environment parser when the file just
        // isn't a collection; real JSON errors should surface as-is.
        Err(PostmanError::NotACollection) => match parse_postman_environment(&text) {
            Ok((env, secrets)) => Ok(Parsed::Environment(env, secrets)),
            Err(_) => Err("Not a Postman collection or environment export".to_string()),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.postman_import.open {
        return;
    }
    let Some(workspace) = state.workspace.clone() else {
        state.dialogs.postman_import.open = false;
        state.status = Some(StatusMessage::error("Open a workspace before importing"));
        return;
    };

    let mut window_open = true;
    let mut import_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Import Postman")
        .id(egui::Id::new("postman-import-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 420.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let dialog = &mut state.dialogs.postman_import;
            match &dialog.parsed {
                None => {
                    ui.weak("No file loaded.");
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(Parsed::Collection(import))) => {
                    ui.label(format!(
                        "Collection \"{}\" — {} request(s), {} collection variable(s)",
                        import.name,
                        import.request_count(),
                        import.variables.len()
                    ));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Import as:");
                        ui.text_edit_singleline(&mut dialog.name);
                    });
                    show_skipped(ui, &import.skipped);
                }
                Some(Ok(Parsed::Environment(env, secrets))) => {
                    ui.label(format!(
                        "Environment \"{}\" — {} variable(s), {} secret value(s)",
                        env.name,
                        env.variables.len(),
                        secrets.len()
                    ));
                    if !secrets.is_empty() {
                        ui.weak(
                            "Secret values go to the gitignored .secrets.json sibling file, \
                             never into the committed environment file.",
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Import as:");
                        ui.text_edit_singleline(&mut dialog.name);
                    });
                }
            }

            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
                let can_import = matches!(&state.dialogs.postman_import.parsed, Some(Ok(_)))
                    && !state.dialogs.postman_import.name.trim().is_empty();
                if ui
                    .add_enabled(can_import, egui::Button::new("Import"))
                    .clicked()
                {
                    import_clicked = true;
                }
            });
        });

    if import_clicked {
        match do_import(&workspace, &mut state.dialogs.postman_import) {
            Err(e) => state.status = Some(StatusMessage::error(e)),
            Ok(msg) => {
                state.dialogs.postman_import.open = false;
                reload_workspace(state);
                state.status = Some(StatusMessage::info(msg));
            }
        }
    }
    if cancel_clicked || !window_open {
        state.dialogs.postman_import.open = false;
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
    egui::ScrollArea::vertical()
        .id_salt("postman_import-sa-1")
        .max_height(160.0)
        .show(ui, |ui| {
            for note in skipped {
                ui.weak(note);
            }
        });
}

fn do_import(workspace: &Workspace, dialog: &mut PostmanImportState) -> Result<String, String> {
    let name = dialog.name.trim().to_string();
    match dialog.parsed.as_ref() {
        Some(Ok(Parsed::Collection(import))) => {
            let count = import.request_count();
            import_collection(workspace, import, &name)?;
            Ok(format!("Imported {count} request(s) from Postman"))
        }
        Some(Ok(Parsed::Environment(env, secrets))) => {
            let file = create_environment(&workspace.root, &name).map_err(|e| e.to_string())?;
            let mut env = env.clone();
            env.name = name.clone();
            save_environment(&file, &env).map_err(|e| e.to_string())?;
            if !secrets.is_empty() {
                save_secrets(&file, secrets).map_err(|e| e.to_string())?;
            }
            Ok(format!("Imported Postman environment \"{name}\""))
        }
        _ => Err("nothing to import".to_string()),
    }
}

pub(crate) fn import_collection(
    workspace: &Workspace,
    import: &ImportedCollection,
    name: &str,
) -> Result<(), String> {
    let col_dir = create_collection(&workspace.root, name).map_err(|e| e.to_string())?;

    let order = write_items(&col_dir, &import.items)?;

    let mut meta = forge_core::model::CollectionMeta::new(name);
    meta.description = import.description.clone();
    meta.variables = import.variables.clone();
    meta.auth = import.auth.clone();
    meta.hooks = import.hooks.clone();
    meta.order = order;
    save_collection_meta(&col_dir, &meta).map_err(|e| e.to_string())?;
    Ok(())
}

/// Write folders/requests into `dir`, returning the child entry names in
/// Postman order for the parent's `order` array.
fn write_items(dir: &Path, items: &[ImportedItem]) -> Result<Vec<String>, String> {
    let mut order = Vec::new();
    for item in items {
        match item {
            ImportedItem::Request(def) => {
                let file = create_request(dir, def).map_err(|e| e.to_string())?;
                order.push(file_name(&file));
            }
            ImportedItem::Folder {
                name,
                description,
                auth,
                hooks,
                items,
            } => {
                let sub = create_folder(dir, name).map_err(|e| e.to_string())?;
                let sub_order = write_items(&sub, items)?;
                let meta = FolderMeta {
                    name: name.clone(),
                    description: description.clone(),
                    auth: auth.clone(),
                    hooks: hooks.clone(),
                    order: sub_order,
                    ..FolderMeta::default()
                };
                save_folder_meta(&sub, &meta).map_err(|e| e.to_string())?;
                order.push(file_name(&sub));
            }
        }
    }
    Ok(order)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn reload_workspace(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        return;
    };
    match Workspace::load(&root) {
        Ok(ws) => state.workspace = Some(ws),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use forge_core::model::{AuthConfig, FolderMeta, RequestDef};
    use forge_core::store::{load_json, Workspace, FOLDER_FILE};

    use super::*;

    /// The full fixture round-trip lives in forge-core; this covers the
    /// GUI-side write path: directory layout, order arrays, folder metadata.
    #[test]
    fn import_collection_writes_tree_order_and_folder_auth() {
        let fixture = include_str!("../../../forge-core/tests/fixtures/postman_collection.json");
        let import = parse_postman(fixture).expect("fixture should parse");

        let dir = tempfile::tempdir().expect("tempdir");
        let ws = Workspace::create(dir.path(), "WS").expect("create workspace");

        import_collection(&ws, &import, "Payments").expect("import should succeed");

        let col_dir = dir.path().join("collections/payments");
        let meta: forge_core::model::CollectionMeta =
            load_json(&col_dir.join("collection.json")).expect("collection meta");
        assert_eq!(meta.name, "Payments");
        assert_eq!(
            meta.variables.get("baseUrl").map(String::as_str),
            Some("https://api.example.com")
        );
        assert!(matches!(meta.auth, AuthConfig::Bearer { .. }));
        // Postman order preserved: Charges folder first, then the three requests.
        assert_eq!(meta.order.len(), 4);
        assert_eq!(meta.order[0], "charges");
        assert!(meta.order[1].starts_with("login"));

        let folder: FolderMeta =
            load_json(&col_dir.join("charges").join(FOLDER_FILE)).expect("folder meta");
        assert_eq!(folder.name, "Charges");
        assert!(matches!(folder.auth, AuthConfig::ApiKey { .. }));
        assert_eq!(folder.order.len(), 2);

        let req: RequestDef =
            load_json(&col_dir.join("charges").join(&folder.order[0])).expect("request file");
        assert_eq!(req.name, "Create Charge");
        assert_eq!(req.url, "{{baseUrl}}/v1/charges");
    }
}
