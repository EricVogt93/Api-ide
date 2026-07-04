//! The top-level [`eframe::App`]: menu bar, tool-window stripes, the
//! collections/environment side panels, the editor tab strip and the
//! status bar. Wires the bridge's async run events back into [`AppState`].

use std::path::PathBuf;

use forge_core::runner::{RunEvent, RunOptions, RunScope};
use forge_core::store::{save_request, Workspace};

use crate::bridge::{Bridge, Cmd, Evt};
use crate::keymap::{self, ActionId};
use crate::panels::{collections, request_editor};
use crate::state::{AppState, RunState, StatusMessage};
use crate::theme::{icons, ThemeKind};

/// The Forge IDE application.
pub struct ForgeApp {
    state: AppState,
    bridge: Bridge,
    about_open: bool,
}

impl ForgeApp {
    /// Construct the app, optionally loading a workspace given on the
    /// command line.
    pub fn new(ctx: egui::Context, initial_workspace: Option<PathBuf>) -> Self {
        let bridge = Bridge::new(ctx);
        let mut state = AppState::new();
        if let Some(path) = initial_workspace {
            match Workspace::load(&path) {
                Ok(ws) => state.workspace = Some(ws),
                Err(e) => state.status = Some(StatusMessage::error(format!("{}: {e}", path.display()))),
            }
        }
        Self { state, bridge, about_open: false }
    }

    fn drain_bridge_events(&mut self) {
        while let Some(evt) = self.bridge.try_recv() {
            match evt {
                Evt::Run { run_id, event } => self.handle_run_event(run_id, event),
                Evt::RunFailed { run_id, error } => {
                    self.clear_run(run_id);
                    self.state.status = Some(StatusMessage::error(error));
                }
            }
        }
    }

    fn handle_run_event(&mut self, run_id: u64, event: RunEvent) {
        match event {
            RunEvent::RunStarted { total, .. } => {
                if self.state.run_state.run_id == Some(run_id) {
                    self.state.run_state.total = total;
                }
            }
            RunEvent::RequestFinished(outcome) => {
                self.state.run_state.completed += 1;
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
                }
                self.state.status =
                    Some(StatusMessage::info(format!("Run finished: {}/{} passed", summary.passed, summary.total)));
            }
            RunEvent::IterationStarted { .. } | RunEvent::RequestStarted { .. } => {}
        }
    }

    fn clear_run(&mut self, run_id: u64) {
        if self.state.run_state.run_id == Some(run_id) {
            self.state.run_state.run_id = None;
        }
        for tab in &mut self.state.tabs {
            if tab.run_id == Some(run_id) {
                tab.run_id = None;
            }
        }
    }

    fn dispatch_action(&mut self, action: ActionId) {
        match action {
            ActionId::Save => {
                if let Some(idx) = self.state.active_tab {
                    save_tab(&mut self.state, idx);
                }
            }
            ActionId::SaveAll => save_all(&mut self.state),
            ActionId::Send => request_editor::send_active(&mut self.state, &self.bridge),
            ActionId::CloseTab => {
                if let Some(idx) = self.state.active_tab {
                    self.state.close_tab(idx);
                }
            }
            ActionId::NextTab => self.state.next_tab(),
            ActionId::PrevTab => self.state.prev_tab(),
            ActionId::OpenWorkspace => self.open_workspace_dialog(),
            ActionId::ToggleCollections => self.state.show_collections = !self.state.show_collections,
        }
    }

    fn open_workspace_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            match Workspace::load(&path) {
                Ok(ws) => {
                    self.state.workspace = Some(ws);
                    self.state.tabs.clear();
                    self.state.active_tab = None;
                    self.state.status = Some(StatusMessage::info(format!("Opened {}", path.display())));
                }
                Err(e) => self.state.status = Some(StatusMessage::error(e.to_string())),
            }
        }
    }

    fn new_workspace_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Workspace".to_string());
            match Workspace::create(&path, &name) {
                Ok(ws) => {
                    self.state.workspace = Some(ws);
                    self.state.tabs.clear();
                    self.state.active_tab = None;
                    self.state.status = Some(StatusMessage::info(format!("Created workspace at {}", path.display())));
                }
                Err(e) => self.state.status = Some(StatusMessage::error(e.to_string())),
            }
        }
    }

    fn run_workspace(&mut self) {
        let Some(ws) = self.state.workspace.clone() else {
            self.state.status = Some(StatusMessage::error("No workspace open"));
            return;
        };
        let run_id = self.state.alloc_run_id();
        self.state.run_state = RunState { run_id: Some(run_id), total: 0, completed: 0 };
        self.bridge.send(Cmd::Run {
            run_id,
            workspace: Box::new(ws),
            scope: RunScope::Workspace,
            options: RunOptions { environment: self.state.active_env.clone(), ..Default::default() },
        });
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
                if ui.checkbox(&mut self.state.show_collections, "Collections").changed() {}
                if ui.checkbox(&mut self.state.show_environment, "Environment").changed() {}
                if ui.checkbox(&mut self.state.show_bottom, "Bottom Tool Window").changed() {}
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
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Forge").clicked() {
                    self.about_open = true;
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
                for (i, tab) in self.state.tabs.iter().enumerate() {
                    let is_active = self.state.active_tab == Some(i);
                    let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 4)).fill(if is_active {
                        ui.visuals().selection.bg_fill.gamma_multiply(0.35)
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
            // Bottom tool-window stripe (disabled placeholders; wave C).
            ui.add_enabled(false, egui::Button::new(icons::RUN)).on_disabled_hover_text("Run (wave C)");
            ui.add_enabled(false, egui::Button::new(icons::HISTORY)).on_disabled_hover_text("History (wave C)");
            ui.add_enabled(false, egui::Button::new(icons::CONSOLE)).on_disabled_hover_text("Console (wave C)");
            ui.add_enabled(false, egui::Button::new(icons::COOKIES)).on_disabled_hover_text("Cookies (wave C)");
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

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = self.about_open;
        egui::Window::new("About Forge").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.label("Forge — an IntelliJ-style API testing IDE.");
            ui.label(format!("forge-gui {}", env!("CARGO_PKG_VERSION")));
        });
        self.about_open = open;
    }
}

impl eframe::App for ForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_bridge_events();

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
                environment_panel(ui, &self.state);
            });
        }

        egui::Panel::bottom("status-bar").exact_size(28.0).resizable(false).show(ui, |ui| {
            self.status_bar(ui);
        });

        egui::CentralPanel::default().show(ui, |ui| {
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

        self.about_window(ui.ctx());
        self.toast(ui);
    }
}

/// Read-only environment variable list for the right tool window; secret
/// values are masked.
fn environment_panel(ui: &mut egui::Ui, state: &AppState) {
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

fn save_tab(state: &mut AppState, idx: usize) {
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

fn save_all(state: &mut AppState) {
    for idx in 0..state.tabs.len() {
        save_tab(state, idx);
    }
}
