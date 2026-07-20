//! Execution history tool window: a filterable table over
//! [`forge_core::history::HistoryStore`] with view and diff modals.
//!
//! The store lives on [`AppState`] (opened lazily per workspace, see
//! [`open_store`]) and is queried synchronously from the GUI thread — SQLite
//! reads here are local and fast enough not to need the bridge thread.

use std::path::Path;

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};

use forge_core::history::{
    diff_entries, DiffResult, HistoryEntry, HistoryFilter, HistoryStore, HistorySummary, NewEntry,
};
use forge_core::model::{BodyDef, Method, RequestDef};
use forge_core::runner::RequestOutcome;
use forge_core::store::Workspace;

use crate::state::{AppState, StatusMessage};
use crate::theme::ThemeKind;
use crate::widgets::method_badge::method_color;

/// File name of the per-workspace history database, under `.forge-local/`.
pub const HISTORY_DB_FILE: &str = "history.sqlite";

/// Open (creating `.forge-local/` if needed) the history database for a
/// workspace rooted at `root`.
pub fn open_store(root: &Path) -> Result<HistoryStore, String> {
    let dir = root.join(forge_core::store::LOCAL_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;
    HistoryStore::open(&dir.join(HISTORY_DB_FILE))
        .map_err(|error| format!("failed to open history: {error}"))
}

/// Build a [`NewEntry`] for recording `outcome` in the history store.
///
/// The response URL comes from the executed request. Request headers and
/// body are copied from the stored definition without resolving templates,
/// so history remains useful without persisting resolved secret values.
pub fn new_entry_from_outcome<'a>(
    workspace: Option<&Workspace>,
    outcome: &'a RequestOutcome,
    env: Option<String>,
) -> NewEntry<'a> {
    let def = workspace.and_then(|ws| ws.find_request(&outcome.id));
    let method = def
        .map(|n| n.def.method.as_str().to_string())
        .unwrap_or_default();
    let url = match &outcome.result {
        Ok(exec) => exec.effective_url.clone(),
        Err(_) => def.map(|n| n.def.url.clone()).unwrap_or_default(),
    };
    NewEntry {
        request_id: outcome.id.clone(),
        name: outcome.name.clone(),
        method,
        url,
        env,
        outcome: match &outcome.result {
            Ok(exec) => Ok(exec),
            Err(e) => Err(e.as_str()),
        },
        request_headers: def
            .map(|node| {
                node.def
                    .headers
                    .iter()
                    .filter(|header| header.is_active())
                    .map(|header| (header.key.clone(), header.value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        request_body: def.and_then(|node| configured_body(&node.def)),
    }
}

fn configured_body(def: &RequestDef) -> Option<Vec<u8>> {
    match &def.body {
        BodyDef::None => None,
        BodyDef::Raw { text, .. } | BodyDef::Json { text } | BodyDef::Xml { text } => {
            Some(text.as_bytes().to_vec())
        }
        BodyDef::GraphQl { query, .. } => Some(query.as_bytes().to_vec()),
        BodyDef::Binary { path } => Some(format!("@{path}").into_bytes()),
        BodyDef::FormUrlencoded { .. } | BodyDef::Multipart { .. } => {
            serde_json::to_vec(&def.body).ok()
        }
    }
}

/// Status-class filter for the history table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    Any,
    Success2xx,
    Redirect3xx,
    ClientError4xx,
    ServerError5xx,
}

impl StatusFilter {
    const ALL: [StatusFilter; 5] = [
        StatusFilter::Any,
        StatusFilter::Success2xx,
        StatusFilter::Redirect3xx,
        StatusFilter::ClientError4xx,
        StatusFilter::ServerError5xx,
    ];

    fn label(&self) -> &'static str {
        match self {
            StatusFilter::Any => "Any status",
            StatusFilter::Success2xx => "2xx",
            StatusFilter::Redirect3xx => "3xx",
            StatusFilter::ClientError4xx => "4xx",
            StatusFilter::ServerError5xx => "5xx",
        }
    }

    fn range(&self) -> (Option<u16>, Option<u16>) {
        match self {
            StatusFilter::Any => (None, None),
            StatusFilter::Success2xx => (Some(200), Some(299)),
            StatusFilter::Redirect3xx => (Some(300), Some(399)),
            StatusFilter::ClientError4xx => (Some(400), Some(499)),
            StatusFilter::ServerError5xx => (Some(500), Some(599)),
        }
    }
}

/// Transient UI state for the history tool window.
#[derive(Default)]
pub struct HistoryUiState {
    pub filter_text: String,
    pub filter_method: Option<Method>,
    pub filter_status: StatusFilter,
    pub rows: Vec<HistorySummary>,
    /// Ids selected for diffing (insertion order, at most 2 — see
    /// [`toggle_diff_selection`]).
    pub selected: Vec<i64>,
    pub view_entry: Option<HistoryEntry>,
    pub diff: Option<(i64, i64, DiffResult)>,
    pub loaded: bool,
}

fn refresh(store: &HistoryStore, ui_state: &mut HistoryUiState) -> Result<(), String> {
    let (status_min, status_max) = ui_state.filter_status.range();
    let filter = HistoryFilter {
        text: if ui_state.filter_text.trim().is_empty() {
            None
        } else {
            Some(ui_state.filter_text.trim().to_string())
        },
        method: ui_state.filter_method.map(|m| m.as_str().to_string()),
        status_min,
        status_max,
        request_id: None,
        limit: 200,
        offset: 0,
    };
    ui_state.rows = store
        .list(&filter)
        .map_err(|error| format!("failed to load history: {error}"))?;
    ui_state.loaded = true;
    Ok(())
}

fn toggle_diff_selection(selected: &mut Vec<i64>, id: i64) {
    if let Some(pos) = selected.iter().position(|&x| x == id) {
        selected.remove(pos);
        return;
    }
    if selected.len() >= 2 {
        selected.remove(0);
    }
    selected.push(id);
}

fn status_color(theme: ThemeKind, status: Option<u16>) -> Color32 {
    match status {
        Some(s) => match s / 100 {
            2 => theme.ok_color(),
            3 => Color32::from_rgb(0x35, 0x92, 0xC4),
            4 => Color32::from_rgb(0xC7, 0x7D, 0x2E),
            5 => theme.error_color(),
            _ => Color32::GRAY,
        },
        None => theme.error_color(),
    }
}

/// Render the History tool window.
pub fn show(ui: &mut Ui, state: &mut AppState) {
    let theme = state.theme;
    // Pull the store out of `state` for the duration of this call: none of
    // `HistoryStore`'s methods borrow `AppState`, so working with an owned
    // local avoids fighting the borrow checker over `state.history_ui`,
    // `state.workspace` and `state.tabs` at the same time.
    let Some(store) = state.history_store.take() else {
        ui.add_space(8.0);
        ui.weak("History is unavailable for this workspace.");
        return;
    };
    let mut operation_error: Option<String> = None;

    if !state.history_ui.loaded {
        if let Err(error) = refresh(&store, &mut state.history_ui) {
            operation_error = Some(error);
        }
    }

    let mut do_refresh = false;
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.history_ui.filter_text);
        egui::ComboBox::from_id_salt("hist-method")
            .selected_text(
                state
                    .history_ui
                    .filter_method
                    .map(|m| m.as_str())
                    .unwrap_or("Any method"),
            )
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(state.history_ui.filter_method.is_none(), "Any method")
                    .clicked()
                {
                    state.history_ui.filter_method = None;
                }
                for m in Method::ALL {
                    if ui
                        .selectable_label(state.history_ui.filter_method == Some(m), m.as_str())
                        .clicked()
                    {
                        state.history_ui.filter_method = Some(m);
                    }
                }
            });
        egui::ComboBox::from_id_salt("hist-status")
            .selected_text(state.history_ui.filter_status.label())
            .show_ui(ui, |ui| {
                for s in StatusFilter::ALL {
                    if ui
                        .selectable_label(state.history_ui.filter_status == s, s.label())
                        .clicked()
                    {
                        state.history_ui.filter_status = s;
                    }
                }
            });
        if ui.button("Search").clicked() {
            do_refresh = true;
        }
        if ui.button("Clear all").clicked() {
            match store.clear() {
                Ok(()) => {
                    state.history_ui.selected.clear();
                    do_refresh = true;
                }
                Err(error) => operation_error = Some(format!("failed to clear history: {error}")),
            }
        }
    });

    if do_refresh {
        if let Err(error) = refresh(&store, &mut state.history_ui) {
            operation_error = Some(error);
        }
    }

    ui.separator();

    let rows = state.history_ui.rows.clone();
    let mut selected = state.history_ui.selected.clone();
    let mut open_tab: Option<String> = None;
    let mut view_id: Option<i64> = None;
    let mut delete_id: Option<i64> = None;

    egui::ScrollArea::vertical()
        .id_salt("history-list-scroll")
        .auto_shrink([false, false])
        .max_height((ui.available_height() - 34.0).max(60.0))
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("history-table")
                .striped(true)
                .column(Column::exact(24.0))
                .column(Column::auto().at_least(70.0))
                .column(Column::auto().at_least(120.0).resizable(true))
                .column(Column::auto().at_least(140.0).resizable(true))
                .column(Column::remainder().at_least(160.0))
                .column(Column::auto().at_least(50.0))
                .column(Column::auto().at_least(70.0))
                .column(Column::auto().at_least(150.0))
                .header(20.0, |mut header| {
                    header.col(|_ui| {});
                    header.col(|ui| {
                        ui.strong("Method");
                    });
                    header.col(|ui| {
                        ui.strong("Time");
                    });
                    header.col(|ui| {
                        ui.strong("Name");
                    });
                    header.col(|ui| {
                        ui.strong("URL");
                    });
                    header.col(|ui| {
                        ui.strong("Status");
                    });
                    header.col(|ui| {
                        ui.strong("Duration");
                    });
                    header.col(|ui| {
                        ui.strong("");
                    });
                })
                .body(|mut body| {
                    for row in &rows {
                        body.row(22.0, |mut r| {
                            r.col(|ui| {
                                let mut checked = selected.contains(&row.id);
                                if ui.checkbox(&mut checked, "").changed() {
                                    toggle_diff_selection(&mut selected, row.id);
                                }
                            });
                            r.col(|ui| {
                                let method = Method::parse(&row.method);
                                match method {
                                    Some(m) => {
                                        ui.label(
                                            RichText::new(m.as_str())
                                                .color(method_color(m))
                                                .monospace()
                                                .strong(),
                                        );
                                    }
                                    None => {
                                        ui.monospace(&row.method);
                                    }
                                }
                            });
                            r.col(|ui| {
                                ui.label(&row.executed_at);
                            });
                            r.col(|ui| {
                                ui.label(&row.name);
                            });
                            r.col(|ui| {
                                ui.weak(&row.url);
                            });
                            r.col(|ui| {
                                let text = row
                                    .status
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| "err".to_string());
                                ui.colored_label(status_color(theme, row.status), text);
                            });
                            r.col(|ui| {
                                ui.label(format!("{} ms", row.duration_ms));
                            });
                            r.col(|ui| {
                                ui.horizontal(|ui| {
                                    if ui.small_button("Open").clicked() {
                                        open_tab = Some(row.request_id.clone());
                                    }
                                    if ui.small_button("View").clicked() {
                                        view_id = Some(row.id);
                                    }
                                    if ui.small_button("\u{2715}").clicked() {
                                        delete_id = Some(row.id);
                                    }
                                });
                            });
                        });
                    }
                });
        });

    ui.horizontal(|ui| {
        let can_diff = selected.len() == 2;
        if ui
            .add_enabled(can_diff, egui::Button::new("Diff selected"))
            .clicked()
        {
            match (store.get(selected[0]), store.get(selected[1])) {
                (Ok(Some(a)), Ok(Some(b))) => {
                    let result = diff_entries(&a, &b);
                    state.history_ui.diff = Some((selected[0], selected[1], result));
                }
                (Ok(None), _) | (_, Ok(None)) => {
                    operation_error = Some("selected history entry no longer exists".to_string());
                }
                (Err(error), _) | (_, Err(error)) => {
                    operation_error = Some(format!("failed to load history diff: {error}"));
                }
            }
        }
        ui.weak(format!("{} of 2 selected for diff", selected.len()));
    });

    state.history_ui.selected = selected;

    let mut needs_refresh = false;
    if let Some(id) = delete_id {
        match store.delete(id) {
            Ok(()) => needs_refresh = true,
            Err(error) => operation_error = Some(format!("failed to delete history: {error}")),
        }
    }
    if let Some(id) = view_id {
        match store.get(id) {
            Ok(entry) => state.history_ui.view_entry = entry,
            Err(error) => operation_error = Some(format!("failed to load history: {error}")),
        }
    }
    if needs_refresh {
        if let Err(error) = refresh(&store, &mut state.history_ui) {
            operation_error = Some(error);
        }
    }

    if let Some(rel_id) = open_tab {
        if let Some(def) = state
            .workspace
            .as_ref()
            .and_then(|ws| ws.find_request(&rel_id).map(|n| n.def.clone()))
        {
            state.open_tab(rel_id, def);
        }
    }

    let ctx = ui.ctx().clone();
    view_modal(&ctx, state);
    diff_modal(&ctx, state);

    if let Some(error) = operation_error {
        state.log.error("history", error.clone());
        state.status = Some(StatusMessage::error(error));
    }
    state.history_store = Some(store);
}

fn view_modal(ctx: &egui::Context, state: &mut AppState) {
    let Some(entry) = state.history_ui.view_entry.clone() else {
        return;
    };
    let mut open = true;
    egui::Window::new(format!("{} {}", entry.method, entry.name))
        .id(egui::Id::new("history-view-modal"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 420.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(format!("{} \u{2014} {}", entry.executed_at, entry.url));
            if let Some(status) = entry.status {
                ui.label(format!("Status: {status}"));
            }
            if let Some(err) = &entry.error {
                ui.colored_label(state.theme.error_color(), err);
            }
            ui.separator();
            ui.strong("Request headers");
            egui::ScrollArea::vertical()
                .id_salt("hist-view-request-headers")
                .max_height(80.0)
                .show(ui, |ui| {
                    for (k, v) in &entry.request_headers {
                        ui.monospace(format!("{k}: {v}"));
                    }
                });
            ui.strong("Request body");
            let request_body = entry
                .request_body
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).into_owned())
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .id_salt("hist-view-request-body")
                .max_height(100.0)
                .show(ui, |ui| {
                    ui.monospace(request_body);
                });
            ui.separator();
            ui.strong("Response headers");
            egui::ScrollArea::vertical()
                .id_salt("hist-view-headers")
                .max_height(120.0)
                .show(ui, |ui| {
                    for (k, v) in &entry.response_headers {
                        ui.monospace(format!("{k}: {v}"));
                    }
                });
            ui.separator();
            ui.strong("Response body");
            let body_text = entry
                .response_body
                .as_deref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .id_salt("hist-view-body")
                .max_height(220.0)
                .show(ui, |ui| {
                    ui.monospace(body_text);
                });
        });
    if open {
        state.history_ui.view_entry = Some(entry);
    }
}

fn diff_modal(ctx: &egui::Context, state: &mut AppState) {
    let Some((a, b, diff)) = state.history_ui.diff.clone() else {
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Diff #{a} vs #{b}"))
        .id(egui::Id::new("history-diff-modal"))
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 420.0])
        .open(&mut open)
        .show(ctx, |ui| {
            if diff.unified.is_empty() {
                ui.weak("Responses are identical.");
                return;
            }
            ui.label(format!("+{} / -{}", diff.added, diff.removed));
            egui::ScrollArea::both()
                .id_salt("history-diff-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for line in diff.unified.lines() {
                        let color = if line.starts_with('+') && !line.starts_with("+++") {
                            Some(state.theme.ok_color())
                        } else if line.starts_with('-') && !line.starts_with("---") {
                            Some(state.theme.error_color())
                        } else {
                            None
                        };
                        match color {
                            Some(c) => {
                                ui.colored_label(c, line);
                            }
                            None => {
                                ui.monospace(line);
                            }
                        }
                    }
                });
        });
    if open {
        state.history_ui.diff = Some((a, b, diff));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::exec::{ExecutionResult, Sizes, TimingBreakdown};
    use forge_core::model::{BodyDef, KeyValue, RequestDef};
    use forge_core::store::{create_collection, create_request};

    fn exec_result(status: u16) -> ExecutionResult {
        ExecutionResult {
            status,
            status_text: String::new(),
            http_version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
            timing: TimingBreakdown::default(),
            size: Sizes::default(),
            effective_url: "https://example.test/things".to_string(),
            redirect_chain: Vec::new(),
            cookies_set: Vec::new(),
            executed_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn new_entry_maps_outcome_fields_without_a_workspace() {
        let outcome = RequestOutcome {
            id: "collections/a/get-thing.request.json".to_string(),
            name: "Get thing".to_string(),
            iteration: 0,
            result: Ok(exec_result(200)),
            assertions: Vec::new(),
            script_log: Vec::new(),
            script_error: None,
            extracted: Vec::new(),
        };
        let entry = new_entry_from_outcome(None, &outcome, Some("dev".to_string()));
        assert_eq!(entry.request_id, "collections/a/get-thing.request.json");
        assert_eq!(entry.name, "Get thing");
        assert_eq!(entry.env.as_deref(), Some("dev"));
        assert_eq!(entry.url, "https://example.test/things");
        assert!(entry.method.is_empty());
        assert!(entry.request_headers.is_empty());
        assert!(entry.request_body.is_none());
        assert!(entry.outcome.is_ok());
    }

    #[test]
    fn new_entry_maps_transport_failure() {
        let outcome = RequestOutcome {
            id: "req".to_string(),
            name: "Failing".to_string(),
            iteration: 0,
            result: Err("connection refused".to_string()),
            assertions: Vec::new(),
            script_log: Vec::new(),
            script_error: None,
            extracted: Vec::new(),
        };
        let entry = new_entry_from_outcome(None, &outcome, None);
        assert_eq!(entry.outcome.unwrap_err(), "connection refused");
        assert!(entry.env.is_none());
    }

    #[test]
    fn new_entry_records_configured_request_without_resolving_secrets() {
        let root = tempfile::tempdir().expect("tempdir");
        Workspace::create(root.path(), "History").expect("workspace");
        let collection = create_collection(root.path(), "Requests").expect("collection");
        let mut def = RequestDef::new("Create", Method::Post, "https://example.test");
        def.headers
            .push(KeyValue::new("Authorization", "Bearer {{token}}"));
        def.body = BodyDef::Json {
            text: r#"{"token":"{{token}}"}"#.to_string(),
        };
        let file = create_request(&collection, &def).expect("request");
        let workspace = Workspace::load(root.path()).expect("reload");
        let request_id = file
            .strip_prefix(root.path())
            .expect("relative")
            .to_string_lossy()
            .replace('\\', "/");
        let outcome = RequestOutcome {
            id: request_id,
            name: "Create".to_string(),
            iteration: 0,
            result: Ok(exec_result(201)),
            assertions: Vec::new(),
            script_log: Vec::new(),
            script_error: None,
            extracted: Vec::new(),
        };

        let entry = new_entry_from_outcome(Some(&workspace), &outcome, None);

        assert_eq!(
            entry.request_headers,
            vec![("Authorization".to_string(), "Bearer {{token}}".to_string())]
        );
        assert_eq!(
            entry.request_body,
            Some(br#"{"token":"{{token}}"}"#.to_vec())
        );
    }

    #[test]
    fn toggle_diff_selection_caps_at_two_dropping_oldest() {
        let mut selected = Vec::new();
        toggle_diff_selection(&mut selected, 1);
        toggle_diff_selection(&mut selected, 2);
        assert_eq!(selected, vec![1, 2]);
        toggle_diff_selection(&mut selected, 3);
        assert_eq!(selected, vec![2, 3]);
        toggle_diff_selection(&mut selected, 2);
        assert_eq!(selected, vec![3]);
    }
}
