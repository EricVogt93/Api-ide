//! reqv1 asset-store browser (left tool window). Discovery-first: an author
//! writing a request looks here to find test data and reusable assets, copy
//! the exact ref string, and spot broken references — without hand-building
//! JSON Pointers. Read-only; the filesystem stays the source of truth.

use std::path::PathBuf;

use egui::{RichText, Ui};
use forge_core::reqv1::index::AssetEntry;
use forge_core::reqv1::{AssetKind, ProjectIndex};
use serde_json::Value;

use crate::state::{AppState, StatusMessage};
use crate::theme::icons;

#[derive(Default)]
pub struct AssetsState {
    /// Project root currently indexed.
    root: Option<PathBuf>,
    index: Option<ProjectIndex>,
    error: Option<String>,
    /// Which asset rows are expanded (by rel_path).
    expanded: std::collections::HashSet<String>,
    /// Base dir for `suggest_ref` — the active request's directory, so a
    /// copied relative ref is correct for what the author is editing.
    base_dir: Option<PathBuf>,
}

impl AssetsState {
    /// (Re)scan `root`. Cheap enough to call on open and on Refresh.
    pub fn load(&mut self, root: PathBuf) {
        match ProjectIndex::scan(&root) {
            Ok(index) => {
                self.index = Some(index);
                self.error = None;
            }
            Err(d) => {
                self.index = None;
                self.error = Some(d.message);
            }
        }
        self.root = Some(root);
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.index.is_some()
    }
}

/// Render the panel. Returns a ref string the user asked to copy (already
/// placed on the clipboard); the caller may also surface a status line.
pub fn show(ui: &mut Ui, state: &mut AppState) {
    // Auto-load from an open v0 workspace root if it looks like a v1 project,
    // otherwise offer a folder picker.
    if state.assets.root.is_none() {
        if let Some(ws) = &state.workspace {
            if ws.root.join("project.json").exists() {
                state.assets.load(ws.root.clone());
            }
        }
    }
    // Track the active request's directory for relative-ref suggestions.
    state.assets.base_dir = active_request_dir(state);

    ui.horizontal(|ui| {
        if ui.button("Open project…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                state.assets.load(dir);
            }
        }
        let can_refresh = state.assets.root.is_some();
        if ui
            .add_enabled(can_refresh, egui::Button::new("Refresh"))
            .clicked()
        {
            if let Some(root) = state.assets.root.clone() {
                state.assets.load(root);
            }
        }
        if let Some(root) = state.assets.root.clone() {
            if ui
                .button("New request")
                .on_hover_text("Author a v1 request")
                .clicked()
            {
                let env = state.active_env.clone();
                state.dialogs.v1_editor.open_new(root, env);
            }
        }
    });

    if let Some(err) = &state.assets.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
        return;
    }
    let Some(index) = &state.assets.index else {
        ui.add_space(8.0);
        ui.weak("Open a folder containing project.json to browse its asset store.");
        return;
    };

    // Broken refs first — the thing an author most needs to see.
    if !index.broken.is_empty() {
        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            RichText::new(format!("{} broken ref(s)", index.broken.len()))
                .color(ui.visuals().error_fg_color),
        )
        .default_open(true)
        .show(ui, |ui| {
            for b in &index.broken {
                ui.label(
                    RichText::new(format!("{} {}", b.request, b.instance_path))
                        .monospace()
                        .small(),
                );
                ui.label(
                    RichText::new(format!("  {} — {}", b.reference, b.message))
                        .small()
                        .weak(),
                );
            }
        });
        ui.separator();
    }

    let base_dir = state.assets.base_dir.clone();
    let mut to_copy: Option<(String, String)> = None; // (ref, kind-of-thing)
    let mut to_open: Option<String> = None;
    let mut to_edit: Option<std::path::PathBuf> = None;
    let active_env = state.active_env.clone();

    egui::ScrollArea::vertical()
        .id_salt("assets-sa-1")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Requests first — click "edit" to open the v1 editor on that file.
            if !index.requests.is_empty() {
                ui.label(RichText::new("requests").strong());
                for r in &index.requests {
                    ui.horizontal(|ui| {
                        let name = r.rel_path.rsplit('/').next().unwrap_or(&r.rel_path);
                        ui.label(name).on_hover_text(&r.id);
                        ui.label(
                            RichText::new(format!("{} ref(s)", r.refs.len()))
                                .small()
                                .weak(),
                        );
                        if ui.small_button("edit").clicked() {
                            to_edit = Some(std::path::PathBuf::from(&r.path));
                        }
                    });
                }
                ui.separator();
            }

            // Group assets by kind.
            let mut current: Option<AssetKind> = None;
            for asset in &index.assets {
                if current != Some(asset.kind) {
                    current = Some(asset.kind);
                    ui.add_space(4.0);
                    ui.label(RichText::new(asset.kind.label()).strong());
                }
                asset_row(
                    ui,
                    index,
                    asset,
                    base_dir.as_deref(),
                    &mut state.assets.expanded,
                    &mut to_copy,
                    &mut to_open,
                );
            }
        });

    if let Some(file) = to_edit {
        if let Err(error) = state.dialogs.v1_editor.open_file(file, active_env) {
            state.status = Some(StatusMessage::error(error));
        }
    }

    if let Some((r, what)) = to_copy {
        ui.ctx().copy_text(r.clone());
        state.status = Some(StatusMessage::info(format!("Copied {what}: {r}")));
    }
    if let Some(path) = to_open {
        let _ = open::that(path);
    }
}

#[allow(clippy::too_many_arguments)]
fn asset_row(
    ui: &mut Ui,
    index: &ProjectIndex,
    asset: &AssetEntry,
    base_dir: Option<&std::path::Path>,
    expanded: &mut std::collections::HashSet<String>,
    to_copy: &mut Option<(String, String)>,
    to_open: &mut Option<String>,
) {
    let base = base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&index.root));
    let base_ref = index.suggest_ref(asset, &base);
    let browsable = asset.kind == AssetKind::Data && asset.data.is_some();

    ui.horizontal(|ui| {
        let is_open = expanded.contains(&asset.rel_path);
        if browsable {
            let tri = if is_open {
                icons::TRIANGLE_DOWN
            } else {
                icons::TRIANGLE_RIGHT
            };
            if ui.small_button(tri).clicked() {
                if is_open {
                    expanded.remove(&asset.rel_path);
                } else {
                    expanded.insert(asset.rel_path.clone());
                }
            }
        } else {
            ui.add_space(18.0);
        }

        // File name + usage badge.
        let name = asset.rel_path.rsplit('/').next().unwrap_or(&asset.rel_path);
        let resp = ui.selectable_label(false, name);
        if resp.clicked() {
            *to_open = Some(asset.path.clone());
        }
        let uses = asset.used_by.len();
        let badge = RichText::new(format!("×{uses}")).small().weak();
        ui.label(badge).on_hover_ui(|ui| {
            if asset.used_by.is_empty() {
                ui.label("unused");
            } else {
                for u in &asset.used_by {
                    ui.label(format!("{} {}", u.request, u.instance_path));
                }
            }
        });

        if ui
            .small_button("copy ref")
            .on_hover_text(base_ref.clone())
            .clicked()
        {
            *to_copy = Some((base_ref.clone(), "ref".to_string()));
        }
    });

    // Browsable JSON tree for data assets: each node yields a pointer ref.
    if browsable && expanded.contains(&asset.rel_path) {
        if let Some(data) = &asset.data {
            ui.indent(&asset.rel_path, |ui| {
                json_node(ui, &base_ref, "", data, to_copy);
            });
        }
    }
}

/// Render a JSON node; clicking "copy" on any node copies
/// `<base_ref>#<pointer>`. Only container children are shown as rows.
fn json_node(
    ui: &mut Ui,
    base_ref: &str,
    pointer: &str,
    node: &Value,
    to_copy: &mut Option<(String, String)>,
) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{pointer}/{}", escape_ptr(k));
                json_leaf_or_branch(ui, base_ref, &child, k, v, to_copy);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let child = format!("{pointer}/{i}");
                json_leaf_or_branch(ui, base_ref, &child, &i.to_string(), v, to_copy);
            }
        }
        _ => {}
    }
}

fn json_leaf_or_branch(
    ui: &mut Ui,
    base_ref: &str,
    pointer: &str,
    key: &str,
    value: &Value,
    to_copy: &mut Option<(String, String)>,
) {
    let full = format!("{base_ref}#{pointer}");
    let is_container = value.is_object() || value.is_array();
    if is_container {
        let count = match value {
            Value::Object(m) => m.len(),
            Value::Array(a) => a.len(),
            _ => 0,
        };
        egui::CollapsingHeader::new(RichText::new(format!("{key}  ({count})")).monospace())
            .id_salt(pointer)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .small_button("copy ref")
                        .on_hover_text(full.clone())
                        .clicked()
                    {
                        *to_copy = Some((full.clone(), "ref".to_string()));
                    }
                    ui.label(RichText::new(pointer).small().weak());
                });
                json_node(ui, base_ref, pointer, value, to_copy);
            });
    } else {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(
                RichText::new(format!("{key}: {}", preview(value)))
                    .monospace()
                    .small(),
            );
            if ui
                .small_button("copy ref")
                .on_hover_text(full.clone())
                .clicked()
            {
                *to_copy = Some((full.clone(), "ref".to_string()));
            }
        });
    }
}

fn preview(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.len() > 40 {
        format!("{}…", &s[..39])
    } else {
        s
    }
}

/// RFC 6901 escaping for a JSON Pointer segment.
fn escape_ptr(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Directory of the currently active request tab, if it is a v1 request file
/// — used to make copied relative refs correct for the file being edited.
fn active_request_dir(state: &AppState) -> Option<PathBuf> {
    let idx = state.active_tab?;
    let tab = state.tabs.get(idx)?;
    let path = PathBuf::from(&tab.rel_id);
    // Tabs store a workspace-relative id; join to the workspace root.
    let ws_root = state.workspace.as_ref().map(|w| w.root.clone())?;
    let abs = ws_root.join(path);
    abs.parent().map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_pointer_segment() {
        assert_eq!(escape_ptr("a/b"), "a~1b");
        assert_eq!(escape_ptr("m~n"), "m~0n");
    }

    #[test]
    fn preview_truncates_long_values() {
        let long = Value::String("x".repeat(80));
        assert!(preview(&long).ends_with('…'));
    }

    #[test]
    fn scanning_the_fixture_project_populates_the_index() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../forge-core/tests/fixtures/reqv1/project");
        let mut st = AssetsState::default();
        st.load(root);
        assert!(st.is_loaded());
        let index = st.index.as_ref().unwrap();
        assert!(index
            .assets
            .iter()
            .any(|a| a.rel_path == "assets/data/users.json"));
    }
}
