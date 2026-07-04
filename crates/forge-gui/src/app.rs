//! The top-level [`eframe::App`]: menu bar, tool-window stripes, the
//! collections/environment side panels, the editor tab strip and the
//! status bar. Wires the bridge's async run events back into [`AppState`].

use std::path::PathBuf;

use forge_core::runner::{RequestOutcome, RunEvent, RunOptions, RunScope};
use forge_core::store::{save_request, Workspace};

use crate::bridge::{Bridge, Cmd, Evt};
use crate::keymap::{self, ActionId};
use crate::local;
use crate::panels::{collections, console, cookies, history, log, problems, request_editor, terminal, test_results};
use crate::state::{AppState, BottomTool, RunState, StatusMessage};
use crate::theme::{icons, ThemeKind};

/// The Forge IDE application.
pub struct ForgeApp {
    state: AppState,
    bridge: Bridge,
}

impl ForgeApp {
    /// Construct the app, optionally loading a workspace given on the
    /// command line.
    pub fn new(ctx: egui::Context, initial_workspace: Option<PathBuf>) -> Self {
        let bridge = Bridge::new(ctx.clone());
        let mut app = Self { state: AppState::new(), bridge };
        if let Some(path) = initial_workspace {
            match Workspace::load(&path) {
                Ok(ws) => {
                    app.state.workspace = Some(ws);
                    app.on_workspace_opened(&ctx);
                    crate::dialogs::welcome::remember_recent(&path);
                }
                Err(e) => app.state.status = Some(StatusMessage::error(format!("{}: {e}", path.display()))),
            }
        }
        app
    }

    /// Side effects that belong to "a workspace just became `self.state.workspace`":
    /// open its history store, ask the bridge to load its persisted cookie
    /// jar, and restore the last saved UI snapshot (open tabs, active
    /// environment/theme, visible tool windows).
    fn on_workspace_opened(&mut self, ctx: &egui::Context) {
        let Some(root) = self.state.workspace.as_ref().map(|w| w.root.clone()) else { return };
        self.state.log.info("workspace", format!("Opened workspace {}", root.display()));
        self.state.history_store = history::open_store(&root);
        self.bridge.send(Cmd::LoadCookies { path: cookies::cookies_path(&root) });
        if let Some(snapshot) = local::load(&root) {
            local::apply(&mut self.state, snapshot);
            self.state.theme.apply(ctx);
        }
    }

    /// Save the outgoing workspace's UI snapshot (if any was open), then
    /// switch to `ws` and run [`Self::on_workspace_opened`] for it.
    fn switch_workspace(&mut self, ws: Workspace, ctx: &egui::Context) {
        if let Some(old_root) = self.state.workspace.as_ref().map(|w| w.root.clone()) {
            local::save(&old_root, &self.state);
        }
        self.state.workspace = Some(ws);
        self.state.tabs.clear();
        self.state.active_tab = None;
        self.state.history_store = None;
        self.on_workspace_opened(ctx);
    }

    /// Record one finished request execution to the workspace's history
    /// store, if it has one open.
    fn record_history(&self, outcome: &RequestOutcome) {
        let Some(store) = self.state.history_store.as_ref() else { return };
        let entry = history::new_entry_from_outcome(self.state.workspace.as_ref(), outcome, self.state.active_env.clone());
        let _ = store.record(entry);
    }

    fn drain_bridge_events(&mut self) {
        while let Some(evt) = self.bridge.try_recv() {
            match evt {
                Evt::Run { run_id, event } => self.handle_run_event(run_id, event),
                Evt::RunFailed { run_id, error } => {
                    self.clear_run(run_id);
                    self.state.log.error("run", error.clone());
                    self.state.status = Some(StatusMessage::error(error));
                }
                Evt::Ws { conn_id, event } => console::handle_ws_event(&mut self.state, conn_id, event),
                Evt::Sse { conn_id, event } => console::handle_sse_event(&mut self.state, conn_id, event),
                Evt::Cookies(cookies) => self.state.cookies_ui.rows = cookies,
            }
        }
    }

    fn handle_run_event(&mut self, run_id: u64, event: RunEvent) {
        if matches!(event, RunEvent::RunStarted { .. }) {
            self.state.run_log.start(run_id);
        }
        self.state.run_log.apply(run_id, &event);
        if let RunEvent::RequestFinished(outcome) = &event {
            self.record_history(outcome);
        }
        match event {
            RunEvent::RunStarted { total, .. } => {
                if self.state.run_state.run_id == Some(run_id) {
                    self.state.run_state.total = total;
                }
            }
            RunEvent::RequestFinished(outcome) => {
                if self.state.run_state.run_id == Some(run_id) {
                    self.state.run_state.completed += 1;
                }
                match &outcome.result {
                    Err(e) => self.state.log.error("run", format!("{}: {e}", outcome.name)),
                    Ok(res) => {
                        let failed = outcome.assertions.iter().filter(|a| !a.passed).count();
                        if failed > 0 {
                            self.state.log.warn(
                                "run",
                                format!("{}: {} of {} assertions failed", outcome.name, failed, outcome.assertions.len()),
                            );
                        } else {
                            self.state.log.info(
                                "run",
                                format!("{} → {} ({} ms)", outcome.name, res.status, res.timing.total.as_millis()),
                            );
                        }
                    }
                }
                if let Some(idx) = self.state.tab_index_for(&outcome.id) {
                    let tab = &mut self.state.tabs[idx];
                    tab.response = Some(*outcome);
                    tab.response_state.sync(tab.response.as_ref());
                    if tab.run_id == Some(run_id) {
                        tab.run_id = None;
                    }
                }
            }
            RunEvent::RunFinished(summary) => {
                if self.state.run_state.run_id == Some(run_id) {
                    self.state.run_state.run_id = None;
                    let text = format!("Run finished: {}/{} passed", summary.passed, summary.total);
                    if summary.failed > 0 {
                        self.state.log.error("run", text.clone());
                        self.state.status = Some(StatusMessage::error(text));
                    } else {
                        self.state.log.info("run", text.clone());
                        self.state.status = Some(StatusMessage::info(text));
                    }
                }
            }
            RunEvent::IterationStarted { .. } | RunEvent::RequestStarted { .. } => {}
        }
    }

    fn clear_run(&mut self, run_id: u64) {
        if self.state.run_state.run_id == Some(run_id) {
            self.state.run_state.run_id = None;
        }
        if self.state.run_log.run_id == Some(run_id) {
            self.state.run_log.run_id = None;
            self.state.run_log.mark_stopped();
        }
        for tab in &mut self.state.tabs {
            if tab.run_id == Some(run_id) {
                tab.run_id = None;
            }
        }
    }

    /// Slim IntelliJ-style tool-window header: title on the left, a hide
    /// ("collapse") button on the right. Returns `true` when the user asked
    /// to collapse the window.
    fn tool_window_header(ui: &mut egui::Ui, title: &str) -> bool {
        let mut collapse = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(egui::RichText::new(icons::COLLAPSE).size(11.0))
                    .on_hover_text("Hide (reopen via stripe / View menu)")
                    .clicked()
                {
                    collapse = true;
                }
            });
        });
        ui.separator();
        collapse
    }

    /// Open (or focus) the tab of a request by workspace-relative id.
    fn open_request_tab(&mut self, rel_id: &str) {
        if let Some(def) =
            self.state.workspace.as_ref().and_then(|ws| ws.find_request(rel_id).map(|n| n.def.clone()))
        {
            self.state.open_tab(rel_id.to_string(), def);
        }
    }

    fn dispatch_action(&mut self, action: ActionId) {
        crate::dialogs::dispatch_action(&mut self.state, &self.bridge, action);
    }

    fn open_workspace_dialog(&mut self) {
        crate::dialogs::open_workspace(&mut self.state);
    }

    fn new_workspace_dialog(&mut self) {
        crate::dialogs::new_workspace(&mut self.state);
    }

    fn run_workspace(&mut self) {
        let Some(ws) = self.state.workspace.clone() else {
            self.state.status = Some(StatusMessage::error("No workspace open"));
            return;
        };
        let options = RunOptions { environment: self.state.active_env.clone(), ..Default::default() };
        self.state.last_run = Some((RunScope::Workspace, options.clone()));
        let run_id = self.state.alloc_run_id();
        self.state.run_state = RunState { run_id: Some(run_id), total: 0, completed: 0 };
        self.state.run_log.start(run_id);
        self.bridge.send(Cmd::Run { run_id, workspace: Box::new(ws), scope: RunScope::Workspace, options });
    }

    /// Build a menu-item button labelled with an [`ActionId`]'s registered
    /// title, showing its keyboard shortcut (if any) on the trailing side.
    fn action_button(ctx: &egui::Context, id: ActionId) -> egui::Button<'static> {
        let title = keymap::ACTIONS.iter().find(|a| a.id == id).map(|a| a.title).unwrap_or("");
        let mut button = egui::Button::new(title);
        if let Some(shortcut) = keymap::shortcut_for(id) {
            button = button.shortcut_text(ctx.format_shortcut(&shortcut));
        }
        button
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                if ui.add(Self::action_button(ui.ctx(), ActionId::OpenWorkspace)).clicked() {
                    self.open_workspace_dialog();
                    ui.close();
                }
                if ui.button("New Workspace...").clicked() {
                    self.new_workspace_dialog();
                    ui.close();
                }
                ui.separator();
                let has_active = self.state.active_tab.is_some();
                if ui.add_enabled(has_active, Self::action_button(ui.ctx(), ActionId::Save)).clicked() {
                    if let Some(idx) = self.state.active_tab {
                        save_tab(&mut self.state, idx);
                    }
                    ui.close();
                }
                if ui.add(Self::action_button(ui.ctx(), ActionId::SaveAll)).clicked() {
                    save_all(&mut self.state);
                    ui.close();
                }
                ui.separator();
                if ui.add(Self::action_button(ui.ctx(), ActionId::ImportCurl)).clicked() {
                    self.state.dialogs.curl_import.open();
                    ui.close();
                }
                if ui.button("Import OpenAPI...").clicked() {
                    self.state.dialogs.openapi_import.open();
                    ui.close();
                }
                ui.separator();
                if ui.add(Self::action_button(ui.ctx(), ActionId::OpenSettings)).clicked() {
                    self.state.dialogs.settings.open = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Run", |ui| {
                let can_send = self.state.active_tab.is_some();
                if ui.add_enabled(can_send, Self::action_button(ui.ctx(), ActionId::Send)).clicked() {
                    request_editor::send_active(&mut self.state, &self.bridge);
                    ui.close();
                }
                let can_run = self.state.workspace.is_some();
                if ui.add_enabled(can_run, egui::Button::new("Run Collection")).clicked() {
                    self.run_workspace();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.state.show_collections, "Collections");
                ui.checkbox(&mut self.state.show_environment, "Environment");
                ui.checkbox(&mut self.state.show_bottom, "Bottom Tool Window");
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    for kind in ThemeKind::ALL {
                        if ui.selectable_label(self.state.theme == kind, kind.label()).clicked() {
                            self.state.theme = kind;
                            kind.apply(ui.ctx());
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button("Manage Environments...").clicked() {
                    let preferred = self.state.active_env.clone();
                    self.state.dialogs.env_editor.open(preferred);
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Forge").clicked() {
                    self.state.dialogs.about_open = true;
                    ui.close();
                }
            });
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut close_idx: Option<usize> = None;
        let mut select_idx: Option<usize> = None;
        egui::ScrollArea::horizontal().id_salt("tab-bar-scroll").show(ui, |ui| {
            ui.horizontal(|ui| {
                let accent = self.state.theme.accent_color();
                for (i, tab) in self.state.tabs.iter().enumerate() {
                    let is_active = self.state.active_tab == Some(i);
                    // New-UI tab look: flat labels, the active tab gets a
                    // subtle fill plus a 2px accent underline.
                    let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)).fill(if is_active {
                        ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.6)
                    } else {
                        egui::Color32::TRANSPARENT
                    });
                    let resp = frame
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                crate::widgets::method_badge::method_badge(ui, tab.def.method);
                                let title = if tab.dirty { format!("{} {}", tab.title(), icons::DIRTY) } else { tab.title().to_string() };
                                ui.label(title);
                                if ui.small_button(icons::CLOSE).clicked() {
                                    close_idx = Some(i);
                                }
                            });
                        })
                        .response;
                    if is_active {
                        let rect = resp.rect;
                        let underline = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 2.0, rect.bottom() - 2.0),
                            egui::pos2(rect.right() - 2.0, rect.bottom()),
                        );
                        ui.painter().rect_filled(underline, 1.0, accent);
                    }
                    let resp = ui.interact(resp.rect, resp.id.with("tab-click"), egui::Sense::click());
                    if resp.clicked() {
                        select_idx = Some(i);
                    }
                    if resp.middle_clicked() {
                        close_idx = Some(i);
                    }
                }
            });
        });
        if let Some(i) = select_idx {
            self.state.active_tab = Some(i);
        }
        if let Some(i) = close_idx {
            self.state.close_tab(i);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Bottom tool-window stripe: click to show/hide; clicking the
            // already-active one hides it (only one tool window at a time).
            for (tool, icon) in [
                (BottomTool::Run, icons::RUN),
                (BottomTool::Problems, icons::PROBLEMS),
                (BottomTool::Terminal, icons::TERMINAL),
                (BottomTool::Log, icons::LOG),
                (BottomTool::History, icons::HISTORY),
                (BottomTool::Console, icons::CONSOLE),
                (BottomTool::Cookies, icons::COOKIES),
            ] {
                let active = self.state.bottom_tool == Some(tool);
                let text = format!("{icon} {}", tool.label());
                if ui.selectable_label(active, text).clicked() {
                    self.state.bottom_tool = if active { None } else { Some(tool) };
                }
            }
            ui.separator();

            let workspace_name =
                self.state.workspace.as_ref().map(|w| w.meta.name.clone()).unwrap_or_else(|| "No workspace".to_string());
            ui.label(workspace_name);

            ui.separator();
            let env_names: Vec<String> = self
                .state
                .workspace
                .as_ref()
                .map(|w| w.environments.iter().map(|e| e.env.name.clone()).collect())
                .unwrap_or_default();
            let selected = self.state.active_env.clone().unwrap_or_else(|| "No Environment".to_string());
            egui::ComboBox::from_id_salt("active-env").selected_text(selected).show_ui(ui, |ui| {
                if ui.selectable_label(self.state.active_env.is_none(), "No Environment").clicked() {
                    self.state.active_env = None;
                }
                for name in env_names {
                    let is_sel = self.state.active_env.as_deref() == Some(name.as_str());
                    if ui.selectable_label(is_sel, &name).clicked() {
                        self.state.active_env = Some(name);
                    }
                }
            });

            if self.state.run_state.is_running() {
                ui.separator();
                ui.spinner();
                ui.label(format!("Running {}/{}", self.state.run_state.completed, self.state.run_state.total));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(self.state.theme.label());
            });
        });
    }

    fn toast(&mut self, ui: &mut egui::Ui) {
        let Some(status) = &self.state.status else { return };
        if status.expired() {
            self.state.status = None;
            return;
        }
        let color = if status.is_error { self.state.theme.error_color() } else { self.state.theme.ok_color() };
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -32.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(egui::Stroke::new(1.0, color))
                    .corner_radius(4u8)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.colored_label(color, &status.text);
                    });
            });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
}

impl eframe::App for ForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_bridge_events();
        if let Some(ws) = self.state.pending_workspace.take() {
            self.switch_workspace(ws, ui.ctx());
        }

        crate::dialogs::handle_global_shortcuts(ui.ctx(), &mut self.state);
        if let Some(action) = keymap::dispatch(ui.ctx()) {
            self.dispatch_action(action);
        }

        egui::Panel::top("menu-bar").resizable(false).show(ui, |ui| {
            self.menu_bar(ui);
        });

        egui::Panel::left("left-stripe").exact_size(26.0).resizable(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                let active = self.state.show_collections;
                if ui
                    .selectable_label(active, icons::COLLECTIONS)
                    .on_hover_text("Collections")
                    .clicked()
                {
                    self.state.show_collections = !self.state.show_collections;
                }
            });
        });

        if self.state.show_collections {
            egui::Panel::left("left-panel").exact_size(280.0).resizable(true).size_range(180.0..=520.0).show(ui, |ui| {
                if Self::tool_window_header(ui, "Collections") {
                    self.state.show_collections = false;
                }
                collections::show(ui, &mut self.state, &self.bridge);
            });
        }

        egui::Panel::right("right-stripe").exact_size(26.0).resizable(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0);
                let active = self.state.show_environment;
                if ui
                    .selectable_label(active, icons::ENVIRONMENT)
                    .on_hover_text("Environment")
                    .clicked()
                {
                    self.state.show_environment = !self.state.show_environment;
                }
            });
        });

        if self.state.show_environment {
            egui::Panel::right("right-panel").exact_size(260.0).resizable(true).size_range(180.0..=480.0).show(ui, |ui| {
                if Self::tool_window_header(ui, "Environment") {
                    self.state.show_environment = false;
                }
                environment_panel(ui, &mut self.state);
            });
        }

        egui::Panel::bottom("status-bar").exact_size(28.0).resizable(false).show(ui, |ui| {
            self.status_bar(ui);
        });

        if self.state.show_bottom {
            if let Some(tool) = self.state.bottom_tool {
                egui::Panel::bottom("bottom-tool-panel").exact_size(260.0).resizable(true).size_range(120.0..=560.0).show(
                    ui,
                    |ui| {
                        if Self::tool_window_header(ui, tool.label()) {
                            self.state.bottom_tool = None;
                            return;
                        }
                        match tool {
                        BottomTool::Run => test_results::show(ui, &mut self.state, &self.bridge),
                        BottomTool::Problems => {
                            if let Some(rel_id) = problems::show(ui, &mut self.state) {
                                self.open_request_tab(&rel_id);
                            }
                        }
                        BottomTool::Terminal => terminal::show(ui, &mut self.state),
                        BottomTool::Log => log::show(ui, &mut self.state),
                        BottomTool::History => history::show(ui, &mut self.state),
                        BottomTool::Console => console::show(ui, &mut self.state, &self.bridge),
                        BottomTool::Cookies => cookies::show(ui, &mut self.state, &self.bridge),
                        }
                    },
                );
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if self.state.workspace.is_none() {
                crate::dialogs::welcome::show(ui, &mut self.state);
                return;
            }
            self.tab_bar(ui);
            ui.separator();
            if self.state.active_tab.is_some() {
                request_editor::show(ui, &mut self.state, &self.bridge);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.weak("Open a request from the Collections panel to get started.");
                });
            }
        });

        crate::dialogs::show(ui.ctx(), &mut self.state, &self.bridge);
        self.toast(ui);
    }

    /// Persist the open workspace's UI snapshot on shutdown (the bridge
    /// thread saves the cookie jar on its own `Cmd::Shutdown`, sent when
    /// `self.bridge` drops right after this returns).
    fn on_exit(&mut self) {
        if let Some(root) = self.state.workspace.as_ref().map(|w| w.root.clone()) {
            local::save(&root, &self.state);
        }
    }
}

/// Read-only environment variable list for the right tool window; secret
/// values are masked, plus a "Manage..." button opening the environment
/// editor dialog.
fn environment_panel(ui: &mut egui::Ui, state: &mut AppState) {
    if ui.button("Manage...").clicked() {
        let preferred = state.active_env.clone();
        state.dialogs.env_editor.open(preferred);
    }
    ui.separator();
    let Some(workspace) = &state.workspace else {
        ui.add_space(8.0);
        ui.weak("No workspace open.");
        return;
    };
    let Some(env_name) = &state.active_env else {
        ui.add_space(8.0);
        ui.weak("No active environment.");
        ui.weak("Select one from the status bar.");
        return;
    };
    let Some(loaded) = workspace.environment(env_name) else {
        ui.weak("Environment not found.");
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("env-vars-grid").num_columns(2).striped(true).show(ui, |ui| {
            ui.strong("Name");
            ui.strong("Value");
            ui.end_row();
            for (name, var) in &loaded.env.variables {
                ui.label(name);
                if var.secret {
                    let has_value = loaded.secrets.contains_key(name);
                    ui.weak(if has_value { "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}" } else { "(not set)" });
                } else {
                    ui.label(var.value.clone().unwrap_or_default());
                }
                ui.end_row();
            }
        });
    });
}

pub(crate) fn save_tab(state: &mut AppState, idx: usize) {
    let Some(root) = state.workspace.as_ref().map(|w| w.root.clone()) else {
        state.status = Some(StatusMessage::error("No workspace open"));
        return;
    };
    let Some(tab) = state.tabs.get_mut(idx) else { return };
    let file = root.join(&tab.rel_id);
    match save_request(&file, &tab.def) {
        Ok(()) => {
            tab.dirty = false;
            state.status = Some(StatusMessage::info(format!("Saved {}", tab.def.name)));
        }
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}

pub(crate) fn save_all(state: &mut AppState) {
    for idx in 0..state.tabs.len() {
        save_tab(state, idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::runner::RunSummary;

    fn dummy_outcome(id: &str) -> RequestOutcome {
        RequestOutcome {
            id: id.to_string(),
            name: "Req".to_string(),
            iteration: 0,
            result: Err("boom".to_string()),
            assertions: Vec::new(),
            script_log: Vec::new(),
            script_error: None,
            extracted: Vec::new(),
        }
    }

    /// A late `RequestFinished`/`RunFinished` from an *older* run (whose
    /// `run_id` no longer matches `run_state.run_id`) must not corrupt the
    /// progress or status toast of the current run.
    #[test]
    fn stale_run_events_do_not_corrupt_current_run_state() {
        let mut app = ForgeApp::new(egui::Context::default(), None);
        app.state.run_state = RunState { run_id: Some(2), total: 5, completed: 1 };

        app.handle_run_event(1, RunEvent::RequestFinished(Box::new(dummy_outcome("req-a"))));
        assert_eq!(
            app.state.run_state.completed, 1,
            "a RequestFinished from a stale run_id must not bump the current run's completed count"
        );

        let summary = RunSummary { total: 1, passed: 1, failed: 0, skipped: 0, duration_ms: 5 };
        app.handle_run_event(1, RunEvent::RunFinished(summary));
        assert_eq!(
            app.state.run_state.run_id,
            Some(2),
            "a RunFinished from a stale run_id must not clear the current run's run_id"
        );
        assert!(
            app.state.status.is_none(),
            "a RunFinished from a stale run_id must not post a status toast for the current run"
        );

        // The current run's own events still apply normally.
        app.handle_run_event(2, RunEvent::RequestFinished(Box::new(dummy_outcome("req-b"))));
        assert_eq!(app.state.run_state.completed, 2);
    }
}
