//! reqv1 request editor: a self-contained window for authoring a
//! `*.request.json` with *chill* access to the asset store. Store palette on
//! the left (data fixtures, hooks, assertions, extractors, generators,
//! mocks) — click "insert" to drop a ready `ref`/`use` snippet into the JSON
//! at the cursor, so you reference a stored dataset/assertion instead of
//! rewriting it. JSON editor on the right with Validate / Save / Run.

use std::path::PathBuf;

use egui::{text::CCursorRange, RichText, TextEdit, Window};
use forge_core::reqv1::index::AssetEntry;
use forge_core::reqv1::{AssetKind, ProjectIndex, RunResult, RunStatus};

use crate::bridge::{Bridge, Cmd};
use crate::state::AppState;

#[derive(Default)]
pub struct V1EditorState {
    pub open: bool,
    /// File being edited (its parent's project root is derived).
    file: Option<PathBuf>,
    root: Option<PathBuf>,
    text: String,
    dirty: bool,
    index: Option<ProjectIndex>,
    /// JSON tree expansion in the palette (by asset rel_path).
    expanded: std::collections::HashSet<String>,
    /// Byte offset of the primary text cursor, for insert-at-cursor.
    cursor_byte: usize,
    env_name: Option<String>,
    mock: bool,
    // Run plumbing.
    next_run_id: u64,
    active_run: Option<u64>,
    in_flight: bool,
    diagnostics: Vec<String>,
    result: Option<RunResult>,
}

impl V1EditorState {
    /// Open the editor on `file` (an existing document). Rescans its project.
    pub fn open_file(&mut self, file: PathBuf, active_env: Option<String>) {
        self.text = std::fs::read_to_string(&file).unwrap_or_default();
        self.root = Some(project_root_of(&file));
        self.env_name = active_env;
        self.load_index();
        self.file = Some(file);
        self.dirty = false;
        self.result = None;
        self.diagnostics.clear();
        self.open = true;
    }

    /// Open a new, unsaved skeleton request in `root`.
    pub fn open_new(&mut self, root: PathBuf, active_env: Option<String>) {
        self.text = SKELETON.to_string();
        self.root = Some(root);
        self.env_name = active_env;
        self.load_index();
        self.file = None;
        self.dirty = true;
        self.result = None;
        self.diagnostics.clear();
        self.open = true;
    }

    fn load_index(&mut self) {
        self.index = self.root.as_ref().and_then(|r| ProjectIndex::scan(r).ok());
    }

    /// Route a bridge `Evt::V1Run` outcome.
    pub fn handle_result(&mut self, run_id: u64, result: Result<RunResult, String>) {
        if self.active_run != Some(run_id) {
            return;
        }
        self.in_flight = false;
        self.active_run = None;
        match result {
            Ok(r) => self.result = Some(r),
            Err(e) => self.diagnostics = vec![e],
        }
    }
}

const SKELETON: &str = r#"{
  "formatVersion": 1,
  "kind": "request",
  "meta": { "id": "new.request", "name": "New request" },
  "bindings": {
  },
  "request": {
    "method": "GET",
    "url": "${env.baseUrl}/",
    "headers": []
  },
  "pipeline": [
    { "phase": "afterResponse", "use": "builtin:assert-status@1", "with": { "expected": 200 } }
  ]
}
"#;

/// Render the window if open.
pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    if !state.dialogs.v1_editor.open {
        return;
    }
    let mut window_open = true;
    let mut insert_snippet: Option<String> = None;

    Window::new("Request (v1)")
        .id(egui::Id::new("v1-editor"))
        .collapsible(false)
        .resizable(true)
        .default_size([960.0, 640.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let d = &mut state.dialogs.v1_editor;
            ui.horizontal(|ui| {
                ui.label(
                    d.file
                        .as_ref()
                        .map(|f| f.file_name().unwrap_or_default().to_string_lossy().into_owned())
                        .unwrap_or_else(|| "unsaved".to_string()),
                );
                if d.dirty {
                    ui.label(RichText::new("●").weak());
                }
            });
            ui.separator();

            ui.columns(2, |cols| {
                // --- left: store palette ---
                cols[0].label(RichText::new("Asset store — insert a reference").strong());
                cols[0].weak("Click “insert” to drop a ref/use snippet at the cursor.");
                egui::ScrollArea::vertical().id_salt("v1-palette").auto_shrink([false, false]).show(
                    &mut cols[0],
                    |ui| palette(ui, d, &mut insert_snippet),
                );

                // --- right: JSON editor + toolbar + results ---
                let ui = &mut cols[1];
                ui.horizontal(|ui| {
                    if ui.button("Validate").clicked() {
                        validate_now(d);
                    }
                    if ui.button("Save").clicked() {
                        save_now(d);
                    }
                    ui.checkbox(&mut d.mock, "mock");
                    let can_run = !d.in_flight && d.root.is_some();
                    if ui.add_enabled(can_run, egui::Button::new("▶ Run")).clicked() {
                        run_now(d, bridge);
                    }
                    if d.in_flight {
                        ui.spinner();
                    }
                });

                let output = egui::ScrollArea::vertical().id_salt("v1-json").max_height(360.0).show(ui, |ui| {
                    let out = TextEdit::multiline(&mut d.text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(20)
                        .show(ui);
                    if out.response.changed() {
                        d.dirty = true;
                    }
                    out
                });
                // Track the cursor byte offset for insert-at-cursor.
                if let Some(range) = output.inner.cursor_range {
                    d.cursor_byte = byte_offset(&d.text, range);
                }

                results_strip(ui, d);
            });
        });

    if let Some(snippet) = insert_snippet {
        let d = &mut state.dialogs.v1_editor;
        let at = d.cursor_byte.min(d.text.len());
        d.text.insert_str(at, &snippet);
        d.cursor_byte = at + snippet.len();
        d.dirty = true;
    }
    if !window_open {
        state.dialogs.v1_editor.open = false;
    }
}

fn palette(ui: &mut egui::Ui, d: &mut V1EditorState, insert: &mut Option<String>) {
    let Some(index) = &d.index else {
        ui.weak("No project index.");
        return;
    };
    let mut current: Option<AssetKind> = None;
    for asset in &index.assets {
        if current != Some(asset.kind) {
            current = Some(asset.kind);
            ui.add_space(4.0);
            ui.label(RichText::new(asset.kind.label()).strong());
        }
        palette_row(ui, asset, &mut d.expanded, insert);
    }
}

fn palette_row(
    ui: &mut egui::Ui,
    asset: &AssetEntry,
    expanded: &mut std::collections::HashSet<String>,
    insert: &mut Option<String>,
) {
    let base_ref = asset.alias.clone().or_else(|| asset.prefix_ref.clone()).unwrap_or_else(|| asset.rel_path.clone());
    let browsable = asset.kind == AssetKind::Data && asset.data.is_some();

    ui.horizontal(|ui| {
        if browsable {
            let open = expanded.contains(&asset.rel_path);
            if ui.small_button(if open { "▾" } else { "▸" }).clicked() {
                if open {
                    expanded.remove(&asset.rel_path);
                } else {
                    expanded.insert(asset.rel_path.clone());
                }
            }
        } else {
            ui.add_space(16.0);
        }
        let name = asset.rel_path.rsplit('/').next().unwrap_or(&asset.rel_path);
        ui.label(name).on_hover_text(&base_ref);
        // Insert the whole-asset snippet (a binding for data, a pipeline
        // entry for executables) — the chill primitive.
        if ui.small_button("insert").on_hover_text("drop a snippet at the cursor").clicked() {
            *insert = Some(snippet_for(asset, &base_ref));
        }
    });

    if browsable && expanded.contains(&asset.rel_path) {
        if let Some(data) = &asset.data {
            ui.indent(&asset.rel_path, |ui| json_nodes(ui, &base_ref, "", data, insert));
        }
    }
}

fn json_nodes(ui: &mut egui::Ui, base_ref: &str, pointer: &str, node: &serde_json::Value, insert: &mut Option<String>) {
    use serde_json::Value;
    let children: Vec<(String, &Value)> = match node {
        Value::Object(m) => m.iter().map(|(k, v)| (escape_ptr(k), v)).collect(),
        Value::Array(a) => a.iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect(),
        _ => return,
    };
    for (key, value) in children {
        let ptr = format!("{pointer}/{key}");
        let full = format!("{base_ref}#{ptr}");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let label = match value {
                Value::Object(m) => format!("{key} {{{}}}", m.len()),
                Value::Array(a) => format!("{key} [{}]", a.len()),
                other => format!("{key}: {}", short(other)),
            };
            ui.label(RichText::new(label).monospace().small());
            if ui.small_button("insert").on_hover_text(full.clone()).clicked() {
                *insert = Some(format!("{{ \"ref\": \"{full}\" }}"));
            }
        });
        if value.is_object() || value.is_array() {
            ui.indent(&ptr, |ui| json_nodes(ui, base_ref, &ptr, value, insert));
        }
    }
}

/// A ready-to-paste snippet for a whole asset: a binding for data, a pipeline
/// entry for executables (phase inferred from kind).
fn snippet_for(asset: &AssetEntry, base_ref: &str) -> String {
    match asset.kind {
        AssetKind::Data => format!("{{ \"ref\": \"{base_ref}\" }}"),
        AssetKind::Generator => format!("{{ \"use\": \"{base_ref}\", \"with\": {{}} }}"),
        AssetKind::Hook => {
            format!("{{ \"phase\": \"beforeRequest\", \"use\": \"{base_ref}\", \"with\": {{}} }}")
        }
        AssetKind::Assertion | AssetKind::Extractor => {
            format!("{{ \"phase\": \"afterResponse\", \"use\": \"{base_ref}\", \"with\": {{}} }}")
        }
        AssetKind::Mock => format!("{{ \"use\": \"{base_ref}\", \"with\": {{}} }}"),
        AssetKind::Executable => format!("\"{base_ref}\""),
    }
}

fn results_strip(ui: &mut egui::Ui, d: &V1EditorState) {
    ui.separator();
    for msg in &d.diagnostics {
        ui.colored_label(ui.visuals().error_fg_color, msg);
    }
    if let Some(r) = &d.result {
        let (label, color) = match r.status {
            RunStatus::Passed => ("PASSED", egui::Color32::from_rgb(0x49, 0x9C, 0x54)),
            RunStatus::Failed => ("FAILED", egui::Color32::from_rgb(0xC7, 0x5A, 0x3B)),
            RunStatus::Error => ("ERROR", ui.visuals().error_fg_color),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(color).strong());
            if let Some(http) = &r.http {
                ui.label(format!("{} · {} ms", http.status, http.time_ms));
            }
        });
        for a in &r.assertions {
            ui.label(format!("{} {}", if a.passed { "✓" } else { "✗" }, a.message));
        }
        for (k, v) in &r.runtime {
            ui.label(RichText::new(format!("→ {k} = {v}")).small().weak());
        }
        for diag in &r.diagnostics {
            ui.label(RichText::new(format!("[{}] {}", diag.code, diag.message)).small().weak());
        }
    }
}

fn validate_now(d: &mut V1EditorState) {
    d.result = None;
    d.diagnostics.clear();
    let (Some(root), Some(file)) = (d.root.clone(), d.file.clone().or_else(|| d.root.clone())) else {
        d.diagnostics = vec!["no project root".to_string()];
        return;
    };
    match forge_core::reqv1::RequestDocument::parse(&d.text) {
        Ok(doc) => {
            let permissive = |_n: &str| Some("<secret>".to_string());
            let env = forge_core::reqv1::load_environment(&root, d.env_name.as_deref())
                .unwrap_or(serde_json::Value::Null);
            match forge_core::reqv1::validate(&doc, &root, &file, env, &permissive) {
                Ok(ir) => d.diagnostics = vec![format!("ok — {} {}", ir.method, ir.url)],
                Err(diags) => {
                    d.diagnostics = diags
                        .iter()
                        .map(|x| {
                            format!("[{}] {} {}", x.code, x.instance_path.clone().unwrap_or_default(), x.message)
                        })
                        .collect();
                }
            }
        }
        Err(e) => d.diagnostics = vec![format!("invalid JSON: {e}")],
    }
}

fn save_now(d: &mut V1EditorState) {
    let path = match &d.file {
        Some(p) => p.clone(),
        None => {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("request", &["json"])
                .set_file_name("new.request.json")
                .save_file()
            else {
                return;
            };
            path
        }
    };
    if std::fs::write(&path, &d.text).is_ok() {
        d.file = Some(path);
        d.dirty = false;
        d.load_index();
    }
}

fn run_now(d: &mut V1EditorState, bridge: &Bridge) {
    let Some(root) = d.root.clone() else { return };
    // Run needs a file path for relative refs; use the saved file, or a
    // temp path under root for an unsaved buffer.
    let file = d.file.clone().unwrap_or_else(|| root.join("__unsaved__.request.json"));
    let run_id = d.next_run_id;
    d.next_run_id += 1;
    d.active_run = Some(run_id);
    d.in_flight = true;
    d.result = None;
    d.diagnostics.clear();
    bridge.send(Cmd::RunV1 {
        run_id,
        root,
        file,
        text: d.text.clone(),
        env_name: d.env_name.clone(),
        mock: d.mock,
    });
}

// ---------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------

fn project_root_of(file: &std::path::Path) -> PathBuf {
    let mut dir = file.parent().map(std::path::Path::to_path_buf);
    while let Some(d) = dir {
        if d.join("project.json").exists() {
            return d;
        }
        dir = d.parent().map(std::path::Path::to_path_buf);
    }
    file.parent().unwrap_or(std::path::Path::new(".")).to_path_buf()
}

fn escape_ptr(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn short(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.len() > 30 {
        format!("{}…", &s[..29])
    } else {
        s
    }
}

/// Byte offset of the primary cursor from a TextEdit's cursor range.
fn byte_offset(text: &str, range: CCursorRange) -> usize {
    let char_idx = range.primary.index.0;
    text.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::reqv1::index::{AssetEntry, Usage};
    use forge_core::reqv1::AssetKind;

    fn asset(kind: AssetKind, alias: &str) -> AssetEntry {
        AssetEntry {
            path: String::new(),
            rel_path: "assets/x".to_string(),
            kind,
            alias: Some(alias.to_string()),
            prefix_ref: None,
            used_by: Vec::<Usage>::new(),
            data: None,
        }
    }

    #[test]
    fn snippet_shapes_per_kind() {
        assert!(snippet_for(&asset(AssetKind::Data, "data:users"), "data:users").contains("\"ref\""));
        assert!(snippet_for(&asset(AssetKind::Hook, "project:hooks/x"), "project:hooks/x")
            .contains("\"phase\": \"beforeRequest\""));
        assert!(snippet_for(&asset(AssetKind::Assertion, "project:assertions/x"), "project:assertions/x")
            .contains("afterResponse"));
    }

    #[test]
    fn byte_offset_handles_multibyte() {
        let text = "aä€b";
        // char 3 = 'b' starts after a(1)+ä(2)+€(3) = byte 6.
        let range = CCursorRange::one(egui::text::CCursor::new(3));
        assert_eq!(byte_offset(text, range), 6);
    }
}
