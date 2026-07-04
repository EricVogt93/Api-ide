//! Per-workspace UI-state persistence: which tabs were open, the active
//! environment/theme and which tool windows were visible, saved to
//! `<workspace>/.forge-local/ui-state.json` on app exit and on workspace
//! switch, and restored the next time the same workspace is opened.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::{AppState, BottomTool};
use crate::theme::ThemeKind;

const UI_STATE_FILE: &str = "ui-state.json";

fn default_true() -> bool {
    true
}

/// A serializable snapshot of the UI-relevant bits of [`AppState`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UiState {
    #[serde(default)]
    pub open_tabs: Vec<String>,
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub active_env: Option<String>,
    #[serde(default)]
    pub theme: String,
    #[serde(default = "default_true")]
    pub show_collections: bool,
    #[serde(default = "default_true")]
    pub show_environment: bool,
    #[serde(default)]
    pub bottom_panel_selected: Option<String>,
}

fn path_for(root: &Path) -> PathBuf {
    root.join(forge_core::store::LOCAL_DIR).join(UI_STATE_FILE)
}

/// Snapshot the bits of `state` worth restoring the next time this
/// workspace is opened.
pub fn capture(state: &AppState) -> UiState {
    UiState {
        open_tabs: state.tabs.iter().map(|t| t.rel_id.clone()).collect(),
        active_tab: state.active_tab.unwrap_or(0),
        active_env: state.active_env.clone(),
        theme: state.theme.label().to_string(),
        show_collections: state.show_collections,
        show_environment: state.show_environment,
        bottom_panel_selected: state.bottom_tool.map(|t| t.label().to_string()),
    }
}

/// Save `state`'s UI snapshot for the workspace at `root`. Best-effort —
/// I/O failures are swallowed since losing UI-state persistence should
/// never block saving or closing a workspace.
pub fn save(root: &Path, state: &AppState) {
    let path = path_for(root);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&capture(state)) {
        let _ = std::fs::write(path, json);
    }
}

/// Load a previously saved UI snapshot for the workspace at `root`, if any.
pub fn load(root: &Path) -> Option<UiState> {
    let text = std::fs::read_to_string(path_for(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Apply a loaded snapshot to freshly-opened `state` — `state.workspace`
/// must already be set. Tabs are reopened by `rel_id`, skipping any request
/// that no longer exists; the theme is set on `state.theme` only (callers
/// still need `ThemeKind::apply` to push it to the `egui::Context`).
pub fn apply(state: &mut AppState, snapshot: UiState) {
    let Some(workspace) = state.workspace.clone() else { return };
    for rel_id in &snapshot.open_tabs {
        if let Some(def) = workspace.find_request(rel_id).map(|n| n.def.clone()) {
            state.open_tab(rel_id.clone(), def);
        }
    }
    if snapshot.active_tab < state.tabs.len() {
        state.active_tab = Some(snapshot.active_tab);
    }
    state.active_env = snapshot.active_env;
    if let Some(kind) = ThemeKind::ALL.into_iter().find(|k| k.label() == snapshot.theme) {
        state.theme = kind;
    }
    state.show_collections = snapshot.show_collections;
    state.show_environment = snapshot.show_environment;
    state.bottom_tool = snapshot.bottom_panel_selected.and_then(|s| BottomTool::ALL.into_iter().find(|t| t.label() == s));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_state_serde_roundtrip() {
        let original = UiState {
            open_tabs: vec!["collections/a/x.request.json".to_string()],
            active_tab: 0,
            active_env: Some("dev".to_string()),
            theme: "Darcula".to_string(),
            show_collections: true,
            show_environment: false,
            bottom_panel_selected: Some("History".to_string()),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: UiState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, original);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let restored: UiState = serde_json::from_str("{}").expect("deserialize");
        assert!(restored.open_tabs.is_empty());
        assert!(restored.show_collections);
        assert!(restored.show_environment);
    }

    #[test]
    fn load_returns_none_for_a_workspace_with_no_saved_state() {
        let dir = std::env::temp_dir().join(format!("forge-gui-local-test-{}-{}", std::process::id(), line!()));
        assert!(load(&dir).is_none());
    }

    #[test]
    fn save_then_load_roundtrips_through_disk() {
        let dir = std::env::temp_dir().join(format!("forge-gui-local-test-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = AppState::new();
        state.theme = ThemeKind::Light;
        state.show_environment = false;
        save(&dir, &state);
        let loaded = load(&dir).expect("just saved");
        assert_eq!(loaded.theme, "Light");
        assert!(!loaded.show_environment);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
