//! IntelliJ-test-runner-style "Run" tool window: renders the live (or last
//! finished) test run as a tree — iterations (when there is more than one)
//! containing requests containing failed assertion leaves — plus a detail
//! pane for whatever node is selected.
//!
//! The tree model ([`RunLog`]) is pure state, folded incrementally from the
//! [`RunEvent`] stream that already flows through the bridge into
//! `app.rs::handle_run_event`. It is kept free of `egui` types so it can be
//! unit tested without a graphics context.

use egui::{Color32, RichText, Ui};

use forge_core::assert::AssertionOutcome;
use forge_core::runner::RunEvent;

use crate::bridge::{Bridge, Cmd};
use crate::state::{AppState, RunState};
use crate::theme::ThemeKind;

/// Status of one request node in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Running,
    Passed,
    Failed,
    /// Still `Running` when the run stopped (cancelled or failed outright)
    /// — see [`RunLog::mark_stopped`].
    Skipped,
}

impl NodeStatus {
    fn icon(self) -> &'static str {
        match self {
            NodeStatus::Running => "\u{25CB}", // ○
            NodeStatus::Passed => "\u{2713}",  // ✓
            NodeStatus::Failed => "\u{2715}",  // ✕
            NodeStatus::Skipped => "\u{25B7}", // ▷
        }
    }
}

fn status_color(theme: ThemeKind, status: NodeStatus) -> Color32 {
    match status {
        NodeStatus::Passed => theme.ok_color(),
        NodeStatus::Failed => theme.error_color(),
        NodeStatus::Running => Color32::from_gray(0x9E),
        NodeStatus::Skipped => Color32::from_rgb(0xC7, 0x7D, 0x2E),
    }
}

/// One executed (or executing) request under an iteration.
#[derive(Debug, Clone)]
pub struct RequestTreeNode {
    /// Workspace-relative request id, so a selected node can open its tab.
    pub id: String,
    pub name: String,
    pub status: NodeStatus,
    pub duration_ms: u64,
    pub response_status: Option<u16>,
    pub assertions: Vec<AssertionOutcome>,
    pub script_log: Vec<String>,
    pub error: Option<String>,
}

/// One iteration's requests (only rendered as its own group when the run
/// has more than one iteration).
#[derive(Debug, Clone, Default)]
pub struct IterationNode {
    pub iteration: usize,
    pub requests: Vec<RequestTreeNode>,
}

/// Which tree node the detail pane is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    Request(usize, usize),
    Assertion(usize, usize, usize),
}

/// The Run tool window's tree model for a single run, rebuilt incrementally
/// from a stream of [`RunEvent`]s.
#[derive(Debug, Clone, Default)]
pub struct RunLog {
    pub run_id: Option<u64>,
    pub iterations: Vec<IterationNode>,
    pub total: usize,
    pub completed: usize,
    pub passed: usize,
    pub failed: usize,
    pub multi_iteration: bool,
    /// When `true`, passed requests are hidden from the tree.
    pub filter_passed: bool,
    pub selected: Option<Selected>,
}

impl RunLog {
    /// Reset the model and start tracking a freshly started run, keeping
    /// the `filter_passed` preference across runs.
    pub fn start(&mut self, run_id: u64) {
        let filter_passed = self.filter_passed;
        *self = RunLog {
            run_id: Some(run_id),
            filter_passed,
            ..Default::default()
        };
    }

    pub fn is_running(&self) -> bool {
        self.run_id.is_some()
    }

    /// All request nodes across every iteration (for the Problems view).
    pub fn requests(&self) -> impl Iterator<Item = &RequestTreeNode> {
        self.iterations.iter().flat_map(|i| i.requests.iter())
    }

    /// Fold one event belonging to `run_id` into the tree. A no-op if this
    /// log isn't currently tracking that run (e.g. a stale event arriving
    /// after `clear()`/a newer run started).
    pub fn apply(&mut self, run_id: u64, event: &RunEvent) {
        if self.run_id != Some(run_id) {
            return;
        }
        match event {
            RunEvent::RunStarted { total, iterations } => {
                self.total = *total;
                self.multi_iteration = *iterations > 1;
            }
            RunEvent::IterationStarted { iteration } => {
                self.iteration_mut(*iteration);
            }
            RunEvent::RequestStarted {
                id,
                name,
                iteration,
            } => {
                let iter_node = self.iteration_mut(*iteration);
                iter_node.requests.push(RequestTreeNode {
                    id: id.clone(),
                    name: name.clone(),
                    status: NodeStatus::Running,
                    duration_ms: 0,
                    response_status: None,
                    assertions: Vec::new(),
                    script_log: Vec::new(),
                    error: None,
                });
            }
            RunEvent::RequestFinished(outcome) => {
                let passed = outcome.passed();
                let iter_node = self.iteration_mut(outcome.iteration);
                if let Some(node) = iter_node
                    .requests
                    .iter_mut()
                    .rev()
                    .find(|r| r.id == outcome.id && r.status == NodeStatus::Running)
                {
                    node.status = if passed {
                        NodeStatus::Passed
                    } else {
                        NodeStatus::Failed
                    };
                    node.assertions = outcome.assertions.clone();
                    node.script_log = outcome.script_log.clone();
                    match &outcome.result {
                        Ok(exec) => {
                            node.duration_ms = exec.timing.total.as_millis() as u64;
                            node.response_status = Some(exec.status);
                        }
                        Err(e) => node.error = Some(e.clone()),
                    }
                    if let Some(script_err) = &outcome.script_error {
                        node.error = Some(script_err.clone());
                    }
                }
                if passed {
                    self.passed += 1;
                } else {
                    self.failed += 1;
                }
                self.completed += 1;
            }
            RunEvent::RunFinished(_) => {
                self.run_id = None;
                self.mark_stopped();
            }
        }
    }

    /// Called when the run stops without every started request finishing
    /// (cancelled, or the bridge reported an outright failure): any node
    /// still `Running` didn't get to report a result.
    pub fn mark_stopped(&mut self) {
        for iter in &mut self.iterations {
            for req in &mut iter.requests {
                if req.status == NodeStatus::Running {
                    req.status = NodeStatus::Skipped;
                }
            }
        }
    }

    fn iteration_mut(&mut self, iteration: usize) -> &mut IterationNode {
        if let Some(pos) = self
            .iterations
            .iter()
            .position(|it| it.iteration == iteration)
        {
            return &mut self.iterations[pos];
        }
        self.iterations.push(IterationNode {
            iteration,
            requests: Vec::new(),
        });
        let idx = self.iterations.len() - 1;
        &mut self.iterations[idx]
    }
}

/// Render the Run tool window: toolbar, progress bar, tree + detail split.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    let theme = state.theme;
    toolbar(ui, state, bridge);
    ui.separator();

    if state.run_log.total > 0 {
        let progress = state.run_log.completed as f32 / state.run_log.total as f32;
        ui.add(egui::ProgressBar::new(progress).text(format!(
            "{}/{} \u{2014} {} passed, {} failed",
            state.run_log.completed,
            state.run_log.total,
            state.run_log.passed,
            state.run_log.failed
        )));
        ui.add_space(4.0);
    }

    let available = ui.available_size();
    let tree_width = (available.x * 0.4).clamp(200.0, 480.0);

    let mut new_selected: Option<Selected> = None;
    let mut open_tab: Option<String> = None;

    let top_down = egui::Layout::top_down(egui::Align::Min);
    ui.horizontal(|ui| {
        // allocate_ui inherits the surrounding (horizontal) layout, so the
        // panes must switch back to top-down explicitly or tree rows flow
        // sideways.
        ui.allocate_ui_with_layout(egui::vec2(tree_width, available.y), top_down, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("run-tree-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    render_tree(ui, state, theme, &mut new_selected, &mut open_tab);
                });
        });
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2((available.x - tree_width - 12.0).max(100.0), available.y),
            top_down,
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("run-detail-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        render_detail(ui, state, theme);
                    });
            },
        );
    });

    if let Some(sel) = new_selected {
        state.run_log.selected = Some(sel);
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
}

fn toolbar(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    ui.horizontal(|ui| {
        let mut filter_passed = state.run_log.filter_passed;
        if ui
            .selectable_label(filter_passed, "Failures only")
            .clicked()
        {
            filter_passed = !filter_passed;
        }
        state.run_log.filter_passed = filter_passed;

        if ui.button("Clear").clicked() {
            state.run_log = RunLog::default();
        }

        let can_rerun =
            state.last_run.is_some() && state.workspace.is_some() && !state.run_log.is_running();
        if ui
            .add_enabled(
                can_rerun,
                egui::Button::new(format!("{} Re-run", crate::theme::icons::PLAY)),
            )
            .clicked()
        {
            rerun_last(state, bridge);
        }

        if state.run_log.is_running()
            && ui
                .button(format!("{} Stop", crate::theme::icons::STOP))
                .clicked()
        {
            if let Some(run_id) = state.run_log.run_id {
                if let Err(error) = bridge.send(Cmd::Cancel { run_id }) {
                    state.status = Some(crate::state::StatusMessage::error(error));
                }
            }
        }
    });
}

fn rerun_last(state: &mut AppState, bridge: &Bridge) {
    let Some((scope, options)) = state.last_run.clone() else {
        return;
    };
    let Some(ws) = state.workspace.clone() else {
        return;
    };
    let run_id = state.alloc_run_id();
    state.run_state = RunState {
        run_id: Some(run_id),
        total: 0,
        completed: 0,
    };
    state.run_log.start(run_id);
    if let Err(error) = bridge.send(Cmd::Run {
        run_id,
        workspace: Box::new(ws),
        scope,
        options,
    }) {
        state.run_state = RunState::default();
        state.run_log.run_id = None;
        state.run_log.mark_stopped();
        state.status = Some(crate::state::StatusMessage::error(error));
    }
}

fn render_tree(
    ui: &mut Ui,
    state: &AppState,
    theme: ThemeKind,
    selected: &mut Option<Selected>,
    open_tab: &mut Option<String>,
) {
    let run_log = &state.run_log;
    if run_log.iterations.iter().all(|it| it.requests.is_empty()) {
        ui.weak("No run yet. Use Run \u{25B6} from a request tab, the collections tree, or the Run menu.");
        return;
    }

    for (iter_idx, iter_node) in run_log.iterations.iter().enumerate() {
        if run_log.multi_iteration {
            ui.add_space(2.0);
            ui.label(RichText::new(format!("Iteration {}", iter_node.iteration + 1)).strong());
        }
        let indent = if run_log.multi_iteration { 14.0 } else { 0.0 };

        for (req_idx, req) in iter_node.requests.iter().enumerate() {
            if run_log.filter_passed && req.status == NodeStatus::Passed {
                continue;
            }
            let is_sel = run_log.selected == Some(Selected::Request(iter_idx, req_idx));
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.colored_label(status_color(theme, req.status), req.status.icon());
                let label =
                    ui.selectable_label(is_sel, format!("{}  ({} ms)", req.name, req.duration_ms));
                if label.clicked() {
                    *selected = Some(Selected::Request(iter_idx, req_idx));
                }
                if label.double_clicked() {
                    *open_tab = Some(req.id.clone());
                }
            });

            if req.status == NodeStatus::Failed {
                for (a_idx, a) in req.assertions.iter().enumerate() {
                    if a.passed {
                        continue;
                    }
                    let is_a_sel =
                        run_log.selected == Some(Selected::Assertion(iter_idx, req_idx, a_idx));
                    ui.horizontal(|ui| {
                        ui.add_space(indent + 20.0);
                        ui.colored_label(theme.error_color(), "\u{2715}");
                        if ui.selectable_label(is_a_sel, &a.summary).clicked() {
                            *selected = Some(Selected::Assertion(iter_idx, req_idx, a_idx));
                        }
                    });
                }
            }
        }
    }
}

fn render_detail(ui: &mut Ui, state: &AppState, theme: ThemeKind) {
    let Some(sel) = state.run_log.selected else {
        ui.weak("Select a request to see its details here.");
        return;
    };
    let (iter_idx, req_idx) = match sel {
        Selected::Request(i, r) => (i, r),
        Selected::Assertion(i, r, _) => (i, r),
    };
    let Some(req) = state
        .run_log
        .iterations
        .get(iter_idx)
        .and_then(|it| it.requests.get(req_idx))
    else {
        ui.weak("No details available.");
        return;
    };

    ui.horizontal(|ui| {
        ui.colored_label(status_color(theme, req.status), req.status.icon());
        ui.label(RichText::new(&req.name).strong());
    });
    ui.weak(&req.id);
    if let Some(status) = req.response_status {
        ui.label(format!("Response status: {status}"));
    }
    ui.label(format!("Duration: {} ms", req.duration_ms));
    if let Some(err) = &req.error {
        ui.colored_label(theme.error_color(), format!("Error: {err}"));
    }

    if !req.assertions.is_empty() {
        ui.separator();
        ui.strong("Assertions");
        for a in &req.assertions {
            ui.horizontal(|ui| {
                if a.passed {
                    ui.colored_label(theme.ok_color(), "\u{2713}");
                } else {
                    ui.colored_label(theme.error_color(), "\u{2715}");
                }
                ui.label(&a.summary);
                if let Some(msg) = &a.message {
                    ui.weak(msg);
                }
            });
        }
    }

    if !req.script_log.is_empty() {
        ui.separator();
        ui.strong("Script log");
        for line in &req.script_log {
            ui.monospace(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::exec::{ExecutionResult, Sizes, TimingBreakdown};
    use forge_core::runner::{RequestOutcome, RunSummary};

    fn exec_result(status: u16) -> ExecutionResult {
        ExecutionResult {
            status,
            status_text: String::new(),
            http_version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
            timing: TimingBreakdown::default(),
            size: Sizes::default(),
            effective_url: "https://example.test".to_string(),
            redirect_chain: Vec::new(),
            cookies_set: Vec::new(),
            executed_at: chrono::Utc::now(),
        }
    }

    fn outcome(id: &str, iteration: usize, passed: bool) -> RequestOutcome {
        RequestOutcome {
            id: id.to_string(),
            name: id.to_string(),
            iteration,
            result: Ok(exec_result(if passed { 200 } else { 500 })),
            assertions: if passed {
                vec![AssertionOutcome::pass("status is 200")]
            } else {
                vec![AssertionOutcome::fail(
                    "status is 200",
                    "expected 200 got 500",
                )]
            },
            script_log: Vec::new(),
            script_error: None,
            extracted: Vec::new(),
        }
    }

    #[test]
    fn builds_tree_from_event_sequence() {
        let mut log = RunLog::default();
        log.start(1);
        log.apply(
            1,
            &RunEvent::RunStarted {
                total: 2,
                iterations: 1,
            },
        );
        log.apply(1, &RunEvent::IterationStarted { iteration: 0 });
        log.apply(
            1,
            &RunEvent::RequestStarted {
                id: "a".into(),
                name: "A".into(),
                iteration: 0,
            },
        );
        log.apply(
            1,
            &RunEvent::RequestFinished(Box::new(outcome("a", 0, true))),
        );
        log.apply(
            1,
            &RunEvent::RequestStarted {
                id: "b".into(),
                name: "B".into(),
                iteration: 0,
            },
        );
        log.apply(
            1,
            &RunEvent::RequestFinished(Box::new(outcome("b", 0, false))),
        );
        log.apply(
            1,
            &RunEvent::RunFinished(RunSummary {
                total: 2,
                passed: 1,
                failed: 1,
                skipped: 0,
                duration_ms: 10,
            }),
        );

        assert!(!log.is_running());
        assert_eq!(log.iterations.len(), 1);
        assert_eq!(log.iterations[0].requests.len(), 2);
        assert_eq!(log.passed, 1);
        assert_eq!(log.failed, 1);
        assert_eq!(log.completed, 2);
        assert_eq!(log.iterations[0].requests[0].status, NodeStatus::Passed);
        assert_eq!(log.iterations[0].requests[1].status, NodeStatus::Failed);
        assert!(!log.iterations[0].requests[1].assertions[0].passed);
    }

    #[test]
    fn multi_iteration_flag_tracks_run_started() {
        let mut log = RunLog::default();
        log.start(1);
        log.apply(
            1,
            &RunEvent::RunStarted {
                total: 4,
                iterations: 2,
            },
        );
        assert!(log.multi_iteration);
        log.apply(1, &RunEvent::IterationStarted { iteration: 0 });
        log.apply(1, &RunEvent::IterationStarted { iteration: 1 });
        assert_eq!(log.iterations.len(), 2);
    }

    #[test]
    fn events_for_a_different_run_id_are_ignored() {
        let mut log = RunLog::default();
        log.start(1);
        log.apply(
            2,
            &RunEvent::RunStarted {
                total: 5,
                iterations: 1,
            },
        );
        assert_eq!(log.total, 0);
    }

    #[test]
    fn mark_stopped_turns_running_nodes_into_skipped() {
        let mut log = RunLog::default();
        log.start(1);
        log.apply(
            1,
            &RunEvent::RunStarted {
                total: 1,
                iterations: 1,
            },
        );
        log.apply(1, &RunEvent::IterationStarted { iteration: 0 });
        log.apply(
            1,
            &RunEvent::RequestStarted {
                id: "a".into(),
                name: "A".into(),
                iteration: 0,
            },
        );
        log.mark_stopped();
        assert_eq!(log.iterations[0].requests[0].status, NodeStatus::Skipped);
    }
}
