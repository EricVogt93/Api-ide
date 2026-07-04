//! Application state: the open workspace, editor tabs and global UI flags.
//!
//! Kept free of any `egui`/`eframe` types so it can be unit tested without a
//! graphics context.

use std::time::Instant;

use forge_core::history::HistoryStore;
use forge_core::model::RequestDef;
use forge_core::runner::{RequestOutcome, RunOptions, RunScope};
use forge_core::store::Workspace;

use crate::panels::collections::CollectionsUiState;
use crate::panels::console::ConsoleState;
use crate::panels::cookies::CookiesUiState;
use crate::panels::history::HistoryUiState;
use crate::panels::test_results::RunLog;
use crate::theme::ThemeKind;
use crate::widgets::response_view::ResponseViewState;

/// Which sub-tab of the request editor is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestSubTab {
    #[default]
    Params,
    Headers,
    Auth,
    Body,
    Assertions,
    Extract,
    Scripts,
    Settings,
}

/// A single open editor tab: a working copy of a request definition plus
/// whatever UI state belongs only to this tab.
pub struct Tab {
    /// Workspace-relative id of the backing `*.request.json` file — the
    /// stable key used for de-duplication, matching against run events, etc.
    pub rel_id: String,
    /// Working copy; edits mutate this directly, `dirty` tracks whether it
    /// has diverged from what's on disk.
    pub def: RequestDef,
    pub dirty: bool,
    pub response: Option<RequestOutcome>,
    pub sub_tab: RequestSubTab,
    pub response_state: ResponseViewState,
    /// Vertical splitter ratio between the request editor and the response
    /// view, in `0.0..=1.0` (fraction given to the top/request side).
    pub split_ratio: f32,
    /// Set to the bridge run id while a Send initiated from this specific
    /// tab is in flight; cleared when its `RequestFinished`/`RunFailed`
    /// event arrives.
    pub run_id: Option<u64>,
}

impl Tab {
    pub fn new(rel_id: impl Into<String>, def: RequestDef) -> Self {
        Self {
            rel_id: rel_id.into(),
            def,
            dirty: false,
            response: None,
            sub_tab: RequestSubTab::default(),
            response_state: ResponseViewState::default(),
            split_ratio: 0.55,
            run_id: None,
        }
    }

    pub fn title(&self) -> &str {
        &self.def.name
    }
}

/// A transient status-bar / toast message.
#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
    pub created: Instant,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false, created: Instant::now() }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true, created: Instant::now() }
    }

    /// Toasts fade out after this long.
    pub fn expired(&self) -> bool {
        self.created.elapsed().as_secs_f32() > 5.0
    }
}

/// Everything about the currently running (or last requested) test run that
/// the UI needs to render progress/cancel affordances.
#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub run_id: Option<u64>,
    pub total: usize,
    pub completed: usize,
}

impl RunState {
    pub fn is_running(&self) -> bool {
        self.run_id.is_some()
    }
}

/// Which bottom tool window (if any) is currently visible — exactly one
/// shows at a time, IntelliJ style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTool {
    Run,
    History,
    Console,
    Cookies,
}

impl BottomTool {
    pub const ALL: [BottomTool; 4] = [BottomTool::Run, BottomTool::History, BottomTool::Console, BottomTool::Cookies];

    pub fn label(&self) -> &'static str {
        match self {
            BottomTool::Run => "Run",
            BottomTool::History => "History",
            BottomTool::Console => "Console",
            BottomTool::Cookies => "Cookies",
        }
    }
}

/// Top-level application state.
pub struct AppState {
    pub workspace: Option<Workspace>,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    pub active_env: Option<String>,
    pub theme: ThemeKind,
    pub show_collections: bool,
    pub show_environment: bool,
    pub show_bottom: bool,
    pub run_state: RunState,
    pub status: Option<StatusMessage>,
    pub collections: CollectionsUiState,
    /// Which bottom tool window is visible, if any.
    pub bottom_tool: Option<BottomTool>,
    /// Tree model for the Run tool window, fed from the bridge's run
    /// events (see `app.rs::handle_run_event`).
    pub run_log: RunLog,
    /// Scope + options of the most recently started run, so the Run tool
    /// window's "re-run" button can repeat it without needing a workspace
    /// tree/tab context.
    pub last_run: Option<(RunScope, RunOptions)>,
    /// Execution history, opened lazily per workspace at
    /// `<workspace>/.forge-local/history.sqlite`.
    pub history_store: Option<HistoryStore>,
    pub history_ui: HistoryUiState,
    pub console: ConsoleState,
    pub cookies_ui: CookiesUiState,
    next_run_id: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            workspace: None,
            tabs: Vec::new(),
            active_tab: None,
            active_env: None,
            theme: ThemeKind::default(),
            show_collections: true,
            show_environment: true,
            show_bottom: true,
            run_state: RunState::default(),
            status: None,
            collections: CollectionsUiState::default(),
            bottom_tool: None,
            run_log: RunLog::default(),
            last_run: None,
            history_store: None,
            history_ui: HistoryUiState::default(),
            console: ConsoleState::default(),
            cookies_ui: CookiesUiState::default(),
            next_run_id: 0,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh, monotonically increasing run id.
    pub fn alloc_run_id(&mut self) -> u64 {
        self.next_run_id += 1;
        self.next_run_id
    }

    /// Open a tab for `rel_id`, or focus it if already open (de-duplicated
    /// by `rel_id`). Returns the tab's index.
    pub fn open_tab(&mut self, rel_id: impl Into<String>, def: RequestDef) -> usize {
        let rel_id = rel_id.into();
        if let Some(idx) = self.tabs.iter().position(|t| t.rel_id == rel_id) {
            self.active_tab = Some(idx);
            return idx;
        }
        self.tabs.push(Tab::new(rel_id, def));
        let idx = self.tabs.len() - 1;
        self.active_tab = Some(idx);
        idx
    }

    /// Close the tab at `idx`, adjusting `active_tab` to a sensible
    /// neighbor. No-op if `idx` is out of range.
    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        self.active_tab = match self.active_tab {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active > idx => Some(active - 1),
            Some(active) if active == idx => Some(active.min(self.tabs.len() - 1)),
            Some(active) => Some(active),
        };
    }

    /// Mark the tab at `idx` dirty (has unsaved changes). No-op if out of
    /// range. The request editor currently sets `tab.dirty` directly at
    /// each edit site (it already holds `&mut Tab`); this method is the
    /// public entry point for callers that only have an index, e.g. a
    /// future undo/redo or external-sync feature.
    #[allow(dead_code)]
    pub fn mark_dirty(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.dirty = true;
        }
    }

    /// Cycle the active tab forward, wrapping around.
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = Some(match self.active_tab {
            Some(i) => (i + 1) % self.tabs.len(),
            None => 0,
        });
    }

    /// Cycle the active tab backward, wrapping around.
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = Some(match self.active_tab {
            Some(0) | None => self.tabs.len() - 1,
            Some(i) => i - 1,
        });
    }

    /// Reserved for callers that need to mutate the active tab without also
    /// needing other `AppState` fields at the same time (panels that also
    /// need `workspace`/`active_env` alongside the tab destructure `state`
    /// directly instead, to keep disjoint borrows — see
    /// `panels::request_editor::show`).
    #[allow(dead_code)]
    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let idx = self.active_tab?;
        self.tabs.get_mut(idx)
    }

    pub fn active_tab_ref(&self) -> Option<&Tab> {
        let idx = self.active_tab?;
        self.tabs.get(idx)
    }

    /// Find the (first) tab index backed by `rel_id`, e.g. to route a run
    /// event's `RequestOutcome` back to the tab that triggered it.
    pub fn tab_index_for(&self, rel_id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.rel_id == rel_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::model::Method;

    fn def(name: &str) -> RequestDef {
        RequestDef::new(name, Method::Get, "https://example.test")
    }

    #[test]
    fn open_tab_dedupes_by_rel_id() {
        let mut state = AppState::new();
        let a = state.open_tab("collections/a/x.request.json", def("X"));
        let b = state.open_tab("collections/a/y.request.json", def("Y"));
        assert_ne!(a, b);
        assert_eq!(state.tabs.len(), 2);

        // Re-opening the same rel_id focuses the existing tab, no duplicate.
        let a_again = state.open_tab("collections/a/x.request.json", def("X (edited elsewhere)"));
        assert_eq!(a_again, a);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, Some(a));
        // The working copy is not clobbered by the dedupe path.
        assert_eq!(state.tabs[a].def.name, "X");
    }

    #[test]
    fn close_tab_adjusts_active_index() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.open_tab("b", def("B"));
        state.open_tab("c", def("C"));
        state.active_tab = Some(2);

        state.close_tab(0);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.tabs[0].rel_id, "b");
        // Active was after the removed tab, so it shifts down by one.
        assert_eq!(state.active_tab, Some(1));
    }

    #[test]
    fn close_active_tab_clamps_to_last() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.open_tab("b", def("B"));
        state.active_tab = Some(1);

        state.close_tab(1);
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.active_tab, Some(0));
    }

    #[test]
    fn close_last_tab_clears_active() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.close_tab(0);
        assert!(state.tabs.is_empty());
        assert_eq!(state.active_tab, None);
    }

    #[test]
    fn close_tab_out_of_range_is_noop() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.close_tab(5);
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn mark_dirty_sets_flag() {
        let mut state = AppState::new();
        let idx = state.open_tab("a", def("A"));
        assert!(!state.tabs[idx].dirty);
        state.mark_dirty(idx);
        assert!(state.tabs[idx].dirty);
    }

    #[test]
    fn mark_dirty_out_of_range_is_noop() {
        let mut state = AppState::new();
        state.mark_dirty(0); // no tabs open; must not panic
    }

    #[test]
    fn next_and_prev_tab_wrap_around() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.open_tab("b", def("B"));
        state.open_tab("c", def("C"));
        state.active_tab = Some(0);

        state.next_tab();
        assert_eq!(state.active_tab, Some(1));
        state.next_tab();
        assert_eq!(state.active_tab, Some(2));
        state.next_tab();
        assert_eq!(state.active_tab, Some(0));

        state.prev_tab();
        assert_eq!(state.active_tab, Some(2));
        state.prev_tab();
        assert_eq!(state.active_tab, Some(1));
    }

    #[test]
    fn next_prev_tab_noop_when_empty() {
        let mut state = AppState::new();
        state.next_tab();
        assert_eq!(state.active_tab, None);
        state.prev_tab();
        assert_eq!(state.active_tab, None);
    }

    #[test]
    fn tab_index_for_finds_by_rel_id() {
        let mut state = AppState::new();
        state.open_tab("a", def("A"));
        state.open_tab("b", def("B"));
        assert_eq!(state.tab_index_for("b"), Some(1));
        assert_eq!(state.tab_index_for("missing"), None);
    }

    #[test]
    fn alloc_run_id_is_monotonic() {
        let mut state = AppState::new();
        let a = state.alloc_run_id();
        let b = state.alloc_run_id();
        assert!(b > a);
    }
}
