//! The collections tree: a hand-rolled indented outline (folders,
//! sub-folders and requests) with expand/collapse triangles, a context
//! menu for CRUD operations, and small `egui::Window` modals for
//! name-input and delete-confirmation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};

use forge_core::convert::{to_curl, CurlExportOptions};
use forge_core::model::{Method, RequestDef};
use forge_core::runner::{RunOptions, RunScope};
use forge_core::store::{
    create_collection, create_folder, create_request, delete_dir, delete_request, duplicate_request, rename_folder,
    rename_request, TreeNode, Workspace,
};

use crate::bridge::{Bridge, Cmd};
use crate::state::{AppState, RunState, StatusMessage};
use crate::theme::icons;
use crate::widgets::method_badge::method_color;

/// Transient UI state for the collections tree (expand/collapse, pending
/// modal). Lives on [`AppState`] but is defined here since it's purely this
/// panel's concern.
#[derive(Default)]
pub struct CollectionsUiState {
    /// Directories that are currently collapsed (default: everything is
    /// expanded, so this starts empty).
    pub collapsed: HashSet<PathBuf>,
    pub pending: Option<PendingAction>,
    pub pending_input: String,
}

/// A CRUD action awaiting confirmation through a modal.
pub enum PendingAction {
    NewCollection,
    NewFolder(PathBuf),
    NewRequest(PathBuf),
    RenameFolder(PathBuf, String),
    RenameRequest(PathBuf, String),
    DeleteDir(PathBuf, String),
    DeleteRequest(PathBuf, String),
}

enum RowKind {
    Collection,
    Folder,
    Request { method: Method },
}

struct Row {
    /// Absolute path on disk (directory for collections/folders, file for
    /// requests).
    path: PathBuf,
    /// Workspace-relative id, precomputed so rendering never needs to
    /// borrow the workspace itself.
    rel_id: String,
    depth: usize,
    name: String,
    kind: RowKind,
}

/// Compute a workspace-relative id the same way [`Workspace::rel_id`] does,
/// without needing a loaded `Workspace` (just root + path are pure inputs).
fn rel_id_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Render the collections tool window content.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        ui.add_space(8.0);
        ui.weak("No workspace open.");
        ui.weak("Use File \u{2192} Open Workspace...");
        return;
    };
    let rows = state.workspace.as_ref().map(|w| flatten(w, &root, &state.collections.collapsed)).unwrap_or_default();

    // Deferred effects, applied after the render loop so we're free to
    // mutate `state` without fighting borrows on it mid-loop.
    let mut toggle: Option<PathBuf> = None;
    let mut open_request: Option<String> = None;
    let mut copy_curl: Option<String> = None;
    let mut run_scope: Option<RunScope> = None;
    let mut new_pending: Option<PendingAction> = None;
    let mut duplicate: Option<PathBuf> = None;
    let mut export_code: Option<String> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for row in &rows {
            ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * 14.0);
                match &row.kind {
                    RowKind::Collection | RowKind::Folder => {
                        let collapsed = state.collections.collapsed.contains(&row.path);
                        let triangle = if collapsed { icons::TRIANGLE_RIGHT } else { icons::TRIANGLE_DOWN };
                        if ui.small_button(triangle).clicked() {
                            toggle = Some(row.path.clone());
                        }
                        let icon = if matches!(row.kind, RowKind::Collection) { "\u{1F4E6}" } else { icons::FOLDER };
                        let label = ui.selectable_label(false, format!("{icon} {}", row.name));
                        if label.clicked() {
                            toggle = Some(row.path.clone());
                        }
                        label.context_menu(|ui| {
                            let is_collection = matches!(row.kind, RowKind::Collection);
                            if ui.button("New Request").clicked() {
                                new_pending = Some(PendingAction::NewRequest(row.path.clone()));
                                ui.close();
                            }
                            if ui.button("New Folder").clicked() {
                                new_pending = Some(PendingAction::NewFolder(row.path.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if !is_collection && ui.button("Rename").clicked() {
                                new_pending = Some(PendingAction::RenameFolder(row.path.clone(), row.name.clone()));
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                new_pending = Some(PendingAction::DeleteDir(row.path.clone(), row.name.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(if is_collection { "Run Collection" } else { "Run Folder" }).clicked() {
                                run_scope = Some(if is_collection {
                                    RunScope::Collection(row.rel_id.clone())
                                } else {
                                    RunScope::Folder(row.rel_id.clone())
                                });
                                ui.close();
                            }
                        });
                    }
                    RowKind::Request { method } => {
                        ui.label(
                            RichText::new(method.as_str()).color(method_color(*method)).monospace().strong().size(11.0),
                        );
                        let label = ui.selectable_label(false, &row.name);
                        if label.clicked() {
                            open_request = Some(row.rel_id.clone());
                        }
                        label.context_menu(|ui| {
                            if ui.button("Rename").clicked() {
                                new_pending = Some(PendingAction::RenameRequest(row.path.clone(), row.name.clone()));
                                ui.close();
                            }
                            if ui.button("Duplicate").clicked() {
                                duplicate = Some(row.path.clone());
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                new_pending = Some(PendingAction::DeleteRequest(row.path.clone(), row.name.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Copy as curl").clicked() {
                                copy_curl = Some(row.rel_id.clone());
                                ui.close();
                            }
                            if ui.button("Export code...").clicked() {
                                export_code = Some(row.rel_id.clone());
                                ui.close();
                            }
                            if ui.button("Run Request").clicked() {
                                run_scope = Some(RunScope::Request(row.rel_id.clone()));
                                ui.close();
                            }
                        });
                    }
                }
            });
        }
    });

    // Root-level "New Collection" via a context menu on the empty area
    // below the tree.
    let empty_area = ui.interact(ui.min_rect(), ui.id().with("collections-empty"), egui::Sense::click());
    empty_area.context_menu(|ui| {
        if ui.button("New Collection").clicked() {
            new_pending = Some(PendingAction::NewCollection);
            ui.close();
        }
    });

    if let Some(path) = toggle {
        if !state.collections.collapsed.insert(path.clone()) {
            state.collections.collapsed.remove(&path);
        }
    }

    if let Some(path) = duplicate {
        if let Err(e) = duplicate_request(&path) {
            state.status = Some(StatusMessage::error(e.to_string()));
        }
        reload_workspace(state);
    }

    if let Some(rel_id) = open_request {
        let found = state
            .workspace
            .as_ref()
            .and_then(|ws| ws.find_request(&rel_id).map(|node| node.def.clone()));
        if let Some(def) = found {
            state.open_tab(rel_id, def);
        }
    }

    if let Some(rel_id) = copy_curl {
        let curl = state
            .workspace
            .as_ref()
            .and_then(|ws| ws.find_request(&rel_id).map(|node| to_curl(&node.def, &CurlExportOptions::default())));
        if let Some(curl) = curl {
            match arboard::Clipboard::new().and_then(|mut c| c.set_text(curl)) {
                Ok(()) => state.status = Some(StatusMessage::info("Copied curl command to clipboard")),
                Err(e) => state.status = Some(StatusMessage::error(format!("clipboard error: {e}"))),
            }
        }
    }

    if let Some(rel_id) = export_code {
        let def = state.workspace.as_ref().and_then(|ws| ws.find_request(&rel_id).map(|node| node.def.clone()));
        if let Some(def) = def {
            state.dialogs.snippet_export.open(def);
        }
    }

    if let Some(scope) = run_scope {
        let cloned = state.workspace.as_ref().cloned();
        if let Some(ws) = cloned {
            let options = RunOptions { environment: state.active_env.clone(), ..Default::default() };
            state.last_run = Some((scope.clone(), options.clone()));
            let run_id = state.alloc_run_id();
            state.run_state = RunState { run_id: Some(run_id), total: 0, completed: 0 };
            state.run_log.start(run_id);
            bridge.send(Cmd::Run { run_id, workspace: Box::new(ws), scope, options });
        }
    }

    if let Some(pending) = new_pending {
        state.collections.pending_input = match &pending {
            PendingAction::RenameFolder(_, name) | PendingAction::RenameRequest(_, name) => name.clone(),
            _ => String::new(),
        };
        state.collections.pending = Some(pending);
    }

    show_modals(ui, state);
}

/// Render whatever modal `state.collections.pending` currently calls for.
fn show_modals(ui: &mut Ui, state: &mut AppState) {
    let ctx = ui.ctx().clone();
    let Some(pending) = state.collections.pending.take() else { return };

    let (title, is_confirm_delete) = match &pending {
        PendingAction::NewCollection => ("New Collection", false),
        PendingAction::NewFolder(_) => ("New Folder", false),
        PendingAction::NewRequest(_) => ("New Request", false),
        PendingAction::RenameFolder(..) => ("Rename", false),
        PendingAction::RenameRequest(..) => ("Rename", false),
        PendingAction::DeleteDir(..) | PendingAction::DeleteRequest(..) => ("Delete", true),
    };

    let mut keep_open = true;
    let mut confirmed = false;
    let mut cancelled = false;

    egui::Window::new(title)
        .id(egui::Id::new("collections-modal"))
        .collapsible(false)
        .resizable(false)
        .open(&mut keep_open)
        .show(&ctx, |ui| {
            if is_confirm_delete {
                let name = match &pending {
                    PendingAction::DeleteDir(_, name) | PendingAction::DeleteRequest(_, name) => name.as_str(),
                    _ => "",
                };
                ui.label(format!("Delete \"{name}\"? This cannot be undone."));
            } else {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut state.collections.pending_input);
                });
            }
            ui.horizontal(|ui| {
                let ok_label = if is_confirm_delete { "Delete" } else { "OK" };
                if ui.button(ok_label).clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

    if confirmed {
        apply_pending(state, pending);
    } else if !cancelled && keep_open {
        // Still open, no decision yet this frame: put it back.
        state.collections.pending = Some(pending);
    }
    // else: cancelled, or the window's own close ("x") button was used —
    // drop the pending action.
}

fn apply_pending(state: &mut AppState, pending: PendingAction) {
    let name = state.collections.pending_input.trim().to_string();
    let root = state.workspace.as_ref().map(|w| w.root.clone());

    let result: Result<Option<(String, String, String)>, String> = (|| {
        match &pending {
            PendingAction::NewCollection => {
                if name.is_empty() {
                    return Ok(None);
                }
                let Some(root) = &root else { return Ok(None) };
                create_collection(root, &name).map_err(|e| e.to_string())?;
            }
            PendingAction::NewFolder(parent) => {
                if name.is_empty() {
                    return Ok(None);
                }
                create_folder(parent, &name).map_err(|e| e.to_string())?;
            }
            PendingAction::NewRequest(parent) => {
                if name.is_empty() {
                    return Ok(None);
                }
                let def = RequestDef::new(&name, Method::Get, "https://");
                create_request(parent, &def).map_err(|e| e.to_string())?;
            }
            PendingAction::RenameFolder(dir, _) => {
                if name.is_empty() {
                    return Ok(None);
                }
                rename_folder(dir, &name).map_err(|e| e.to_string())?;
            }
            PendingAction::RenameRequest(file, _) => {
                if name.is_empty() {
                    return Ok(None);
                }
                let old_rel = root.as_ref().map(|r| rel_id_of(r, file));
                let new_file = rename_request(file, &name).map_err(|e| e.to_string())?;
                if let Some(old_rel) = old_rel {
                    let new_rel = root.as_ref().map(|r| rel_id_of(r, &new_file)).unwrap_or_else(|| old_rel.clone());
                    return Ok(Some((old_rel, new_rel, name.clone())));
                }
            }
            PendingAction::DeleteDir(dir, _) => {
                delete_dir(dir).map_err(|e| e.to_string())?;
            }
            PendingAction::DeleteRequest(file, _) => {
                delete_request(file).map_err(|e| e.to_string())?;
            }
        }
        Ok(None)
    })();

    let succeeded = result.is_ok();

    match result {
        Ok(Some((old_rel, new_rel, new_name))) => {
            if let Some(tab) = state.tabs.iter_mut().find(|t| t.rel_id == old_rel) {
                tab.rel_id = new_rel;
                tab.def.name = new_name;
            }
        }
        Ok(None) => {}
        Err(e) => state.status = Some(StatusMessage::error(e)),
    }

    // Deleting a request/folder doesn't go through the rename tuple above
    // (there's no "new" location), so close any tab(s) for the deleted
    // path directly here.
    if succeeded {
        if let Some(root) = &root {
            match &pending {
                PendingAction::DeleteRequest(file, _) => {
                    let rel = rel_id_of(root, file);
                    close_tabs_matching(state, |r| r == rel);
                }
                PendingAction::DeleteDir(dir, _) => {
                    let prefix = format!("{}/", rel_id_of(root, dir));
                    close_tabs_matching(state, |r| r.starts_with(&prefix));
                }
                _ => {}
            }
        }
    }

    reload_workspace(state);
}

/// Close every open tab whose `rel_id` satisfies `predicate` (used to close
/// tabs for a request/folder that was just deleted), keeping `active_tab`
/// consistent for each removal. Closing from the highest index down means
/// earlier indices in `idxs` are never invalidated by an earlier removal —
/// same invariant `AppState::close_tab` relies on for a single index.
fn close_tabs_matching(state: &mut AppState, predicate: impl Fn(&str) -> bool) {
    let mut idxs: Vec<usize> =
        state.tabs.iter().enumerate().filter(|(_, t)| predicate(&t.rel_id)).map(|(i, _)| i).collect();
    idxs.sort_unstable_by(|a, b| b.cmp(a));
    for idx in idxs {
        state.close_tab(idx);
    }
}

/// Reload the workspace from disk after a mutating operation, keeping open
/// tabs (their in-memory working copies are untouched).
fn reload_workspace(state: &mut AppState) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else { return };
    match Workspace::load(&root) {
        Ok(ws) => state.workspace = Some(ws),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}

fn flatten(workspace: &Workspace, root: &Path, collapsed: &HashSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    for col in &workspace.collections {
        rows.push(Row {
            rel_id: rel_id_of(root, &col.dir),
            path: col.dir.clone(),
            depth: 0,
            name: col.meta.name.clone(),
            kind: RowKind::Collection,
        });
        if !collapsed.contains(&col.dir) {
            flatten_children(&col.children, root, 1, collapsed, &mut rows);
        }
    }
    rows
}

fn flatten_children(children: &[TreeNode], root: &Path, depth: usize, collapsed: &HashSet<PathBuf>, out: &mut Vec<Row>) {
    for child in children {
        let name = child.display_name();
        match child {
            TreeNode::Folder(f) => {
                out.push(Row { rel_id: rel_id_of(root, &f.dir), path: f.dir.clone(), depth, name, kind: RowKind::Folder });
                if !collapsed.contains(&f.dir) {
                    flatten_children(&f.children, root, depth + 1, collapsed, out);
                }
            }
            TreeNode::Request(r) => {
                out.push(Row {
                    rel_id: rel_id_of(root, &r.file),
                    path: r.file.clone(),
                    depth,
                    name,
                    kind: RowKind::Request { method: r.def.method },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::store::create_folder;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("forge-gui-collections-test-{}-{}-{}", std::process::id(), tag, line!()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn deleting_a_request_closes_its_open_tab() {
        let dir = temp_dir("del-req");
        Workspace::create(&dir, "WS").expect("create workspace");
        let col_dir = create_collection(&dir, "Coll").expect("create collection");
        let file =
            create_request(&col_dir, &RequestDef::new("A", Method::Get, "https://a.example.com")).expect("create a");

        let workspace = Workspace::load(&dir).expect("load workspace");
        let rel = workspace.rel_id(&file);

        let mut state = AppState::new();
        state.workspace = Some(workspace);
        state.open_tab(rel.clone(), RequestDef::new("A", Method::Get, "https://a.example.com"));
        assert_eq!(state.tabs.len(), 1);

        apply_pending(&mut state, PendingAction::DeleteRequest(file, "A".to_string()));

        assert!(state.tabs.is_empty(), "the tab for the deleted request should have been closed");
        assert_eq!(state.active_tab, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_folder_closes_every_tab_under_it_but_not_siblings() {
        let dir = temp_dir("del-folder");
        Workspace::create(&dir, "WS").expect("create workspace");
        let col_dir = create_collection(&dir, "Coll").expect("create collection");
        let folder_dir = create_folder(&col_dir, "Sub").expect("create folder");
        let file_in = create_request(&folder_dir, &RequestDef::new("Inside", Method::Get, "https://in.example.com"))
            .expect("create inside");
        let file_out = create_request(&col_dir, &RequestDef::new("Outside", Method::Get, "https://out.example.com"))
            .expect("create outside");

        let workspace = Workspace::load(&dir).expect("load workspace");
        let rel_in = workspace.rel_id(&file_in);
        let rel_out = workspace.rel_id(&file_out);

        let mut state = AppState::new();
        state.workspace = Some(workspace);
        state.open_tab(rel_in.clone(), RequestDef::new("Inside", Method::Get, "https://in.example.com"));
        state.open_tab(rel_out.clone(), RequestDef::new("Outside", Method::Get, "https://out.example.com"));
        state.active_tab = Some(1);
        assert_eq!(state.tabs.len(), 2);

        apply_pending(&mut state, PendingAction::DeleteDir(folder_dir, "Sub".to_string()));

        assert_eq!(state.tabs.len(), 1, "only the tab under the deleted folder should have been closed");
        assert_eq!(state.tabs[0].rel_id, rel_out);
        assert_eq!(state.active_tab, Some(0));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
