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
    // `snapshot.active_tab` is an index into `snapshot.open_tabs`, but any
    // request that no longer exists was silently skipped above, so
    // `state.tabs` can be shorter (and differently ordered relative to
    // gaps) than `snapshot.open_tabs`. Resolve the *rel_id* the snapshot
    // meant to focus, then find wherever that rel_id landed in the
    // restored tabs, instead of reusing the original index.
    state.active_tab = snapshot
        .open_tabs
        .get(snapshot.active_tab)
        .and_then(|rel_id| state.tab_index_for(rel_id))
        .or_else(|| if state.tabs.is_empty() { None } else { Some(state.tabs.len() - 1) });
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

    fn workspace_with_a_and_c(dir: &Path) -> (forge_core::store::Workspace, String, String) {
        use forge_core::model::{Method, RequestDef};
        use forge_core::store::{create_collection, create_request, Workspace};

        let _ = std::fs::remove_dir_all(dir);
        Workspace::create(dir, "WS").expect("create workspace");
        let col_dir = create_collection(dir, "Coll").expect("create collection");
        let file_a =
            create_request(&col_dir, &RequestDef::new("A", Method::Get, "https://a.example.com")).expect("create a");
        let file_c =
            create_request(&col_dir, &RequestDef::new("C", Method::Get, "https://c.example.com")).expect("create c");

        let workspace = Workspace::load(dir).expect("load workspace");
        let rel_a = workspace.rel_id(&file_a);
        let rel_c = workspace.rel_id(&file_c);
        (workspace, rel_a, rel_c)
    }

    /// A snapshot's `active_tab` is an index into `open_tabs`, but any
    /// request that no longer exists on disk is silently skipped while
    /// reopening tabs — so `state.tabs` can end up shorter, with every tab
    /// after the missing one shifted down by one. Reusing the original
    /// index would then focus the *next* tab over instead of the one the
    /// snapshot actually meant.
    #[test]
    fn active_tab_resolves_by_rel_id_not_by_stale_index() {
        let dir = std::env::temp_dir().join(format!("forge-gui-local-test-{}-{}", std::process::id(), line!()));
        let (workspace, rel_a, rel_c) = workspace_with_a_and_c(&dir);
        let missing_rel_b = "collections/coll/b.request.json".to_string(); // never created

        let mut state = AppState::new();
        state.workspace = Some(workspace);
        let snapshot = UiState {
            open_tabs: vec![missing_rel_b, rel_a.clone(), rel_c],
            active_tab: 1, // meant to focus `rel_a`
            active_env: None,
            theme: String::new(),
            show_collections: true,
            show_environment: true,
            bottom_panel_selected: None,
        };

        apply(&mut state, snapshot);

        // Only A and C got reopened, in that order (B was skipped), so A is
        // now at index 0 — not 1 like the stale snapshot index would imply.
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, Some(0));
        assert_eq!(state.tabs[state.active_tab.unwrap()].rel_id, rel_a);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the snapshot's intended active tab is itself the one that no
    /// longer exists, fall back sensibly (the last reopened tab) rather
    /// than leaving `active_tab` pointing nowhere or out of range.
    #[test]
    fn active_tab_falls_back_to_last_tab_when_intended_tab_is_missing() {
        let dir = std::env::temp_dir().join(format!("forge-gui-local-test-{}-{}", std::process::id(), line!()));
        let (workspace, rel_a, rel_c) = workspace_with_a_and_c(&dir);
        let missing_rel_b = "collections/coll/b.request.json".to_string();

        let mut state = AppState::new();
        state.workspace = Some(workspace);
        let snapshot = UiState {
            open_tabs: vec![rel_a, missing_rel_b, rel_c.clone()],
            active_tab: 1, // meant to focus the now-missing request
            active_env: None,
            theme: String::new(),
            show_collections: true,
            show_environment: true,
            bottom_panel_selected: None,
        };

        apply(&mut state, snapshot);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, Some(1));
        assert_eq!(state.tabs[1].rel_id, rel_c);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
