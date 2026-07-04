//! Suite lifecycle hooks editor ("Edit Hooks..." on a collection or folder
//! in the collections tree): four code editors (Before All / Before Each /
//! After Each / After All) plus a scripting-language picker, with explicit
//! Save/Cancel — the same draft-copy model `dialogs::settings` uses for
//! workspace HTTP settings, since these are persisted to
//! `collection.json`/`folder.json` rather than applied live.

use std::path::{Path, PathBuf};

use egui::{Ui, Window};

use forge_core::model::SuiteHooks;
use forge_core::store::{save_collection_meta, save_folder_meta, Workspace, COLLECTION_FILE, FOLDER_FILE};

use crate::panels::request_editor::language_combo;
use crate::state::{AppState, StatusMessage};
use crate::widgets::code_editor::{code_editor, Lang};

/// Transient state of the hooks editor, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct HooksEditorState {
    /// `None` when closed. `Some((dir, is_collection, draft))` while open —
    /// `dir` is the collection/folder directory being edited.
    target: Option<Target>,
}

struct Target {
    dir: PathBuf,
    is_collection: bool,
    draft: SuiteHooks,
}

impl HooksEditorState {
    /// Open the editor for the collection or folder at `dir`, loading its
    /// current hooks as the initial draft.
    pub fn open(&mut self, dir: PathBuf, is_collection: bool, hooks: SuiteHooks) {
        self.target = Some(Target { dir, is_collection, draft: hooks });
    }

    fn is_open(&self) -> bool {
        self.target.is_some()
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.hooks_editor.is_open() {
        return;
    }

    let mut window_open = true;
    let mut save_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Edit Hooks")
        .id(egui::Id::new("hooks-editor-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 520.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let Some(target) = state.dialogs.hooks_editor.target.as_mut() else { return };

            ui.label(format!(
                "Hooks for {} \"{}\"",
                if target.is_collection { "collection" } else { "folder" },
                target.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            ));
            ui.add_space(4.0);
            language_combo(ui, "hooks-editor-lang", &mut target.draft.language);
            ui.add_space(6.0);

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                hook_field(ui, "hooks-before-all", "Before All (once per run)", &mut target.draft.before_all);
                ui.add_space(6.0);
                hook_field(ui, "hooks-before-each", "Before Each request", &mut target.draft.before_each);
                ui.add_space(6.0);
                hook_field(ui, "hooks-after-each", "After Each request", &mut target.draft.after_each);
                ui.add_space(6.0);
                hook_field(ui, "hooks-after-all", "After All (once per run)", &mut target.draft.after_all);
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save_clicked = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
            });
        });

    if save_clicked {
        if let Some(target) = state.dialogs.hooks_editor.target.take() {
            if let Err(e) = persist(&target.dir, target.is_collection, &target.draft) {
                state.status = Some(StatusMessage::error(e));
            } else {
                reload_workspace(state);
            }
        }
    } else if cancel_clicked || !window_open {
        state.dialogs.hooks_editor.target = None;
    }
}

/// One hook's code editor, with a checkbox-style enable toggle folded into
/// "non-empty text = hook set" (same convention the request editor's own
/// Scripts tab uses for `pre_request`/`post_response`).
fn hook_field(ui: &mut Ui, id_salt: &str, label: &str, hook: &mut Option<String>) {
    ui.label(label);
    let mut text = hook.clone().unwrap_or_default();
    if code_editor(ui, id_salt, &mut text, Lang::Plain, None, false, 4, true).changed() {
        *hook = if text.is_empty() { None } else { Some(text) };
    }
}

fn persist(dir: &Path, is_collection: bool, hooks: &SuiteHooks) -> Result<(), String> {
    if is_collection {
        let meta_path = dir.join(COLLECTION_FILE);
        let mut meta = forge_core::store::load_json::<forge_core::model::CollectionMeta>(&meta_path)
            .map_err(|e| e.to_string())?;
        meta.hooks = hooks.clone();
        save_collection_meta(dir, &meta).map_err(|e| e.to_string())
    } else {
        let meta_path = dir.join(FOLDER_FILE);
        let mut meta = forge_core::store::load_json::<forge_core::model::FolderMeta>(&meta_path)
            .unwrap_or_default();
        meta.hooks = hooks.clone();
        save_folder_meta(dir, &meta).map_err(|e| e.to_string())
    }
}

fn reload_workspace(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else { return };
    match Workspace::load(&root) {
        Ok(ws) => state.workspace = Some(ws),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}
