//! Modal dialogs beyond the simple name-input/confirm windows already
//! inlined in `panels::collections`: Settings, Search Everywhere, curl/
//! OpenAPI import, code export, the environment manager, About and the
//! empty-workspace Welcome pane.
//!
//! Every dialog's transient UI state hangs off a single [`DialogManager`]
//! embedded in [`AppState`], and every `show`/`dispatch` function here takes
//! `&mut AppState` (never `&mut self`) — the same shape `panels::collections`
//! already uses — so `app.rs` never has to juggle a second mutable borrow of
//! the state a dialog needs to read and mutate (workspace, tabs, status...).

pub mod about;
pub mod bruno_import;
pub mod curl_import;
pub mod env_editor;
pub mod grpc_call;
pub mod hooks_editor;
pub mod openapi_import;
pub mod postman_import;
pub mod search;
pub mod settings;
pub mod snippet_export;
pub mod v1_editor;
pub mod welcome;

use forge_core::store::Workspace;

use crate::bridge::Bridge;
use crate::keymap::ActionId;
use crate::panels::request_editor;
use crate::state::{AppState, StatusMessage};

/// All dialog-local UI state, owned by [`AppState`].
#[derive(Default)]
pub struct DialogManager {
    pub about_open: bool,
    pub settings: settings::SettingsState,
    pub search: search::SearchState,
    pub curl_import: curl_import::CurlImportState,
    pub openapi_import: openapi_import::OpenApiImportState,
    pub postman_import: postman_import::PostmanImportState,
    pub bruno_import: bruno_import::BrunoImportState,
    pub snippet_export: snippet_export::SnippetExportState,
    pub env_editor: env_editor::EnvEditorState,
    pub hooks_editor: hooks_editor::HooksEditorState,
    pub grpc_call: grpc_call::GrpcCallState,
    pub v1_editor: v1_editor::V1EditorState,
}

/// Render whichever overlay dialogs are currently open. Call once per frame;
/// each dialog internally no-ops when its own `open` flag is `false`. The
/// Welcome pane is not included here — it replaces the central panel's
/// content directly (see `dialogs::welcome::show`) rather than overlaying it.
pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    about::show(ctx, state);
    settings::show(ctx, state);
    search::show(ctx, state, bridge);
    curl_import::show(ctx, state);
    openapi_import::show(ctx, state);
    postman_import::show(ctx, state);
    bruno_import::show(ctx, state);
    snippet_export::show(ctx, state);
    env_editor::show(ctx, state);
    hooks_editor::show(ctx, state);
    grpc_call::show(ctx, state, bridge);
    v1_editor::show(ctx, state, bridge);
}

/// Detect the global dialog-opening gestures that don't fit a single
/// [`egui::KeyboardShortcut`] in the `keymap` registry — currently just a
/// bare double `Shift` press for Search Everywhere. Call once per frame,
/// before `keymap::dispatch`.
pub fn handle_global_shortcuts(ctx: &egui::Context, state: &mut AppState) {
    if search::detect_double_shift(ctx, &mut state.dialogs.search) {
        state.dialogs.search.open(false);
    }
}

/// Execute a registered [`ActionId`]. Shared by the keyboard-shortcut
/// dispatcher in `app.rs` and Search Everywhere's Actions section (selecting
/// an action there routes through this exact function).
pub fn dispatch_action(state: &mut AppState, bridge: &Bridge, action: ActionId) {
    match action {
        ActionId::Save => {
            if let Some(idx) = state.active_tab {
                crate::app::save_tab(state, idx);
            }
        }
        ActionId::SaveAll => crate::app::save_all(state),
        ActionId::Send => request_editor::send_active(state, bridge),
        ActionId::CloseTab => {
            if let Some(idx) = state.active_tab {
                state.close_tab(idx);
            }
        }
        ActionId::NextTab => state.next_tab(),
        ActionId::PrevTab => state.prev_tab(),
        ActionId::OpenWorkspace => open_workspace(state),
        ActionId::ToggleCollections => state.show_collections = !state.show_collections,
        ActionId::OpenSettings => state.dialogs.settings.open = true,
        ActionId::ImportCurl => state.dialogs.curl_import.open(),
        ActionId::SearchActions => state.dialogs.search.open(true),
    }
}

/// Open a workspace via a folder picker, replacing whatever is currently
/// loaded. Shared by the File menu, the `Ctrl+O` shortcut and the Welcome
/// pane's "Open Workspace..." button.
pub fn open_workspace(state: &mut AppState) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        match Workspace::load(&path) {
            Ok(ws) => {
                // Handed to `ForgeApp` at the top of the next frame, which
                // runs the full switch flow (history store, cookie load,
                // UI-state restore) — see `app.rs`.
                state.pending_workspace = Some(ws);
                state.status = Some(StatusMessage::info(format!("Opened {}", path.display())));
                welcome::remember_recent(&path);
            }
            Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
        }
    }
}

/// Create a fresh workspace via a folder picker. Shared by the File menu and
/// the Welcome pane's "New Workspace..." button.
pub fn new_workspace(state: &mut AppState) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Workspace".to_string());
        match Workspace::create(&path, &name) {
            Ok(ws) => {
                state.pending_workspace = Some(ws);
                state.status = Some(StatusMessage::info(format!(
                    "Created workspace at {}",
                    path.display()
                )));
                welcome::remember_recent(&path);
            }
            Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
        }
    }
}
