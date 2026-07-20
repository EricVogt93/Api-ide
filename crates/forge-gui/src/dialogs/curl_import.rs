//! Import curl (File menu / `Ctrl+Shift+V`): paste a curl command line, get a
//! live preview, pick where it lands in the collections tree, then create
//! the request.

use egui::{RichText, TextEdit, Ui, Window};

use forge_core::convert::parse_curl;
use forge_core::model::RequestDef;
use forge_core::store::{create_request, TreeNode, Workspace};

use crate::state::{AppState, StatusMessage};
use crate::widgets::method_badge::method_color;

/// One selectable target directory in the collection/folder picker.
struct TargetDir {
    /// Display label, indented to show nesting (e.g. `Petstore / Auth`).
    label: String,
    path: std::path::PathBuf,
}

/// Transient state of the curl-import dialog, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct CurlImportState {
    open: bool,
    command: String,
    name: String,
    target_idx: usize,
}

impl CurlImportState {
    /// Open the dialog with an empty paste area.
    pub fn open(&mut self) {
        self.open = true;
        self.command.clear();
        self.name.clear();
        self.target_idx = 0;
    }
}

fn target_dirs(workspace: &Workspace) -> Vec<TargetDir> {
    let mut out = Vec::new();
    for col in &workspace.collections {
        out.push(TargetDir {
            label: col.meta.name.clone(),
            path: col.dir.clone(),
        });
        collect_folders(&col.children, &col.meta.name, &mut out);
    }
    out
}

fn collect_folders(children: &[TreeNode], prefix: &str, out: &mut Vec<TargetDir>) {
    for child in children {
        if let TreeNode::Folder(f) = child {
            let label = format!("{prefix} / {}", child.display_name());
            out.push(TargetDir {
                label: label.clone(),
                path: f.dir.clone(),
            });
            collect_folders(&f.children, &label, out);
        }
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.curl_import.open {
        return;
    }
    let Some(workspace) = state.workspace.clone() else {
        // Nothing sensible to import into; drop the dialog rather than show
        // a picker with no options.
        state.dialogs.curl_import.open = false;
        state.status = Some(StatusMessage::error("Open a workspace before importing"));
        return;
    };

    let targets = target_dirs(&workspace);
    let parsed = parse_curl(&state.dialogs.curl_import.command);
    if let Ok(def) = &parsed {
        if state.dialogs.curl_import.name.is_empty() {
            state.dialogs.curl_import.name = def.name.clone();
        }
    }

    let mut window_open = true;
    let mut import_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Import curl")
        .id(egui::Id::new("curl-import-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 420.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            ui.label("Paste a curl command:");
            ui.add(
                TextEdit::multiline(&mut state.dialogs.curl_import.command)
                    .desired_rows(6)
                    .font(egui::FontSelection::from(egui::FontId::monospace(14.0)))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);
            ui.separator();
            ui.label("Preview:");
            match &parsed {
                Ok(def) => preview(ui, def),
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                }
            }
            ui.add_space(8.0);
            ui.separator();

            if targets.is_empty() {
                ui.weak("No collections yet — create one first from the Collections panel.");
            } else {
                ui.horizontal(|ui| {
                    ui.label("Target:");
                    let idx = state.dialogs.curl_import.target_idx.min(targets.len() - 1);
                    egui::ComboBox::from_id_salt("curl-import-target")
                        .selected_text(
                            targets
                                .get(idx)
                                .map(|t| t.label.as_str())
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (i, t) in targets.iter().enumerate() {
                                ui.selectable_value(
                                    &mut state.dialogs.curl_import.target_idx,
                                    i,
                                    &t.label,
                                );
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut state.dialogs.curl_import.name);
                });
            }

            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
                let can_import = parsed.is_ok()
                    && !targets.is_empty()
                    && !state.dialogs.curl_import.name.trim().is_empty();
                if ui
                    .add_enabled(can_import, egui::Button::new("Import"))
                    .clicked()
                {
                    import_clicked = true;
                }
            });
        });

    if import_clicked {
        if let Ok(mut def) = parsed {
            def.name = state.dialogs.curl_import.name.trim().to_string();
            let idx = state
                .dialogs
                .curl_import
                .target_idx
                .min(targets.len().saturating_sub(1));
            if let Some(target) = targets.get(idx) {
                match create_request(&target.path, &def) {
                    Ok(file) => {
                        let rel_id = workspace.rel_id(&file);
                        reload_and_open(state, rel_id, def);
                        state.dialogs.curl_import.open = false;
                    }
                    Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
                }
            }
        }
    }
    if cancel_clicked || !window_open {
        state.dialogs.curl_import.open = false;
    }
}

fn preview(ui: &mut Ui, def: &RequestDef) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(def.method.as_str())
                .color(method_color(def.method))
                .monospace()
                .strong(),
        );
        ui.monospace(&def.url);
    });
    ui.label(format!(
        "{} header(s)",
        def.headers.iter().filter(|h| h.is_active()).count()
    ));
    ui.label(format!("Body: {}", body_kind_label(&def.body)));
}

fn body_kind_label(body: &forge_core::model::BodyDef) -> &'static str {
    use forge_core::model::BodyDef;
    match body {
        BodyDef::None => "none",
        BodyDef::Json { .. } => "JSON",
        BodyDef::Xml { .. } => "XML",
        BodyDef::Raw { .. } => "raw",
        BodyDef::FormUrlencoded { .. } => "form url-encoded",
        BodyDef::Multipart { .. } => "multipart",
        BodyDef::GraphQl { .. } => "GraphQL",
        BodyDef::Binary { .. } => "binary",
    }
}

/// Reload the workspace from disk (picking up the newly written request
/// file) and open a tab for it.
fn reload_and_open(state: &mut AppState, rel_id: String, def: RequestDef) {
    if let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) {
        match Workspace::load(&root) {
            Ok(ws) => state.workspace = Some(ws),
            Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
        }
    }
    state.open_tab(rel_id, def);
    state.status = Some(StatusMessage::info("Imported curl command"));
}
