//! Import OpenAPI (File menu): pick a JSON/YAML spec, choose which
//! operations to bring in, and generate a whole collection — requests,
//! optional contract-test assertions and the spec-to-collection binding.

use std::path::PathBuf;

use egui::{RichText, Window};

use forge_core::openapi::{build_binding, contract_checks, operation_to_request, parse_spec, ParsedSpec};
use forge_core::store::{create_collection, create_request, save_collection_meta, Workspace, SPECS_DIR};

use crate::state::{AppState, StatusMessage};
use crate::widgets::method_badge::method_color;

/// Transient state of the OpenAPI-import dialog, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct OpenApiImportState {
    open: bool,
    spec_path: Option<PathBuf>,
    spec: Option<Result<ParsedSpec, String>>,
    collection_name: String,
    generate_contract: bool,
    copy_spec: bool,
    /// Parallel to `spec.operations`: whether each op is selected for import.
    selected: Vec<bool>,
}

impl OpenApiImportState {
    /// Open a file picker for a JSON/YAML OpenAPI document and parse it
    /// immediately so the dialog can show a summary/operations table.
    pub fn open(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("OpenAPI spec", &["json", "yaml", "yml"])
            .pick_file()
        else {
            return;
        };
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        let parsed = text.and_then(|t| parse_spec(&t).map_err(|e| e.to_string()));
        self.collection_name = match &parsed {
            Ok(spec) if !spec.title.is_empty() => spec.title.clone(),
            _ => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        self.selected = match &parsed {
            Ok(spec) => vec![true; spec.operations.len()],
            Err(_) => Vec::new(),
        };
        self.spec_path = Some(path);
        self.spec = Some(parsed);
        self.generate_contract = true;
        self.copy_spec = true;
        self.open = true;
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.openapi_import.open {
        return;
    }
    let Some(workspace) = state.workspace.clone() else {
        state.dialogs.openapi_import.open = false;
        state.status = Some(StatusMessage::error("Open a workspace before importing"));
        return;
    };

    let mut window_open = true;
    let mut import_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Import OpenAPI")
        .id(egui::Id::new("openapi-import-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([680.0, 520.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let dialog = &mut state.dialogs.openapi_import;
            match &dialog.spec {
                None => {
                    ui.weak("No spec loaded.");
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(spec)) => {
                    ui.label(format!(
                        "{} {} — {} server(s), {} operation(s)",
                        spec.title,
                        spec.version,
                        spec.servers.len(),
                        spec.operations.len()
                    ));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Collection name:");
                        ui.text_edit_singleline(&mut dialog.collection_name);
                    });
                    ui.checkbox(&mut dialog.generate_contract, "Generate contract assertions");
                    ui.checkbox(&mut dialog.copy_spec, "Copy spec into workspace specs/ dir");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Select all").clicked() {
                            dialog.selected.iter_mut().for_each(|s| *s = true);
                        }
                        if ui.button("Select none").clicked() {
                            dialog.selected.iter_mut().for_each(|s| *s = false);
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        egui::Grid::new("openapi-ops-grid").num_columns(4).striped(true).spacing([10.0, 4.0]).show(
                            ui,
                            |ui| {
                                ui.strong("");
                                ui.strong("Method");
                                ui.strong("Path");
                                ui.strong("Operation");
                                ui.end_row();
                                for (i, op) in spec.operations.iter().enumerate() {
                                    if let Some(sel) = dialog.selected.get_mut(i) {
                                        ui.checkbox(sel, "");
                                    }
                                    ui.label(RichText::new(op.method.as_str()).color(method_color(op.method)).monospace().strong());
                                    ui.monospace(&op.path);
                                    let label = if op.summary.is_empty() { op.id.clone() } else { format!("{} ({})", op.summary, op.id) };
                                    ui.label(label);
                                    ui.end_row();
                                }
                            },
                        );
                    });
                }
            }

            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
                let can_import = matches!(&state.dialogs.openapi_import.spec, Some(Ok(_)))
                    && state.dialogs.openapi_import.selected.iter().any(|s| *s)
                    && !state.dialogs.openapi_import.collection_name.trim().is_empty();
                if ui.add_enabled(can_import, egui::Button::new("Import")).clicked() {
                    import_clicked = true;
                }
            });
        });

    if import_clicked {
        if let Err(e) = do_import(&workspace, &mut state.dialogs.openapi_import) {
            state.status = Some(StatusMessage::error(e));
        } else {
            state.dialogs.openapi_import.open = false;
            reload_workspace(state);
            state.status = Some(StatusMessage::info("OpenAPI spec imported"));
        }
    }
    if cancel_clicked || !window_open {
        state.dialogs.openapi_import.open = false;
    }
}

fn do_import(workspace: &Workspace, dialog: &mut OpenApiImportState) -> Result<(), String> {
    let Some(Ok(spec)) = &dialog.spec else { return Err("no spec loaded".to_string()) };
    let Some(spec_source) = &dialog.spec_path else { return Err("no spec file".to_string()) };

    let col_dir = create_collection(&workspace.root, dialog.collection_name.trim()).map_err(|e| e.to_string())?;

    let spec_rel_path = if dialog.copy_spec {
        let specs_dir = workspace.root.join(SPECS_DIR);
        std::fs::create_dir_all(&specs_dir).map_err(|e| e.to_string())?;
        let file_name = spec_source.file_name().map(|n| n.to_owned()).unwrap_or_else(|| "spec.yaml".into());
        let dest = specs_dir.join(&file_name);
        std::fs::copy(spec_source, &dest).map_err(|e| e.to_string())?;
        format!("{SPECS_DIR}/{}", file_name.to_string_lossy())
    } else {
        spec_source.display().to_string()
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    for (i, op) in spec.operations.iter().enumerate() {
        if !dialog.selected.get(i).copied().unwrap_or(false) {
            continue;
        }
        let mut req = operation_to_request(op);
        if dialog.generate_contract {
            req.assertions = contract_checks(op, None)
                .into_iter()
                .map(|check| {
                    let mut def: forge_core::model::AssertionDef = check.into();
                    def.note = "contract".to_string();
                    def
                })
                .collect();
        }
        let file = create_request(&col_dir, &req).map_err(|e| e.to_string())?;
        let rel_to_collection = file.strip_prefix(&col_dir).unwrap_or(&file).to_string_lossy().replace('\\', "/");
        pairs.push((rel_to_collection, op.id.clone()));
    }

    let mut meta = forge_core::model::CollectionMeta::new(dialog.collection_name.trim());
    meta.openapi = Some(build_binding(&spec_rel_path, &pairs));
    save_collection_meta(&col_dir, &meta).map_err(|e| e.to_string())?;

    Ok(())
}

fn reload_workspace(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else { return };
    match Workspace::load(&root) {
        Ok(ws) => state.workspace = Some(ws),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}
