//! The top-level [`eframe::App`]: menu bar, tool-window stripes, the
//! collections/environment side panels, the editor tab strip and the
//! status bar. Wires the bridge's async run events back into [`AppState`].

use std::path::PathBuf;

use forge_core::runner::{RequestOutcome, RunEvent, RunOptions, RunScope};
use forge_core::store::{save_request, Workspace};

use crate::bridge::{Bridge, Cmd, Evt};
use crate::keymap::{self, ActionId};
use crate::local;
use crate::panels::{
    assets, collections, console, cookies, history, log, problems, request_editor, terminal,
    test_results, variables,
};
use crate::state::{AppState, BottomTool, RunState, StatusMessage};
use crate::theme::{icons, ThemeKind};

pub(crate) const LIGHT_WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/logo-light.png");
pub(crate) const DARK_WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/logo-dark.png");

fn window_icon_png(theme: egui::Theme) -> &'static [u8] {
    match theme {
        egui::Theme::Light => LIGHT_WINDOW_ICON_PNG,
        egui::Theme::Dark => DARK_WINDOW_ICON_PNG,
    }
}

/// The ApiWright IDE application.
pub struct ForgeApp {
    state: AppState,
    bridge: Bridge,
    window_icon_theme: Option<egui::Theme>,
    next_update_check: std::time::Instant,
}

impl ForgeApp {
    /// Construct the app, optionally loading a workspace given on the
    /// command line.
    pub fn new(ctx: egui::Context, initial_workspace: Option<PathBuf>) -> Self {
        let bridge = Bridge::new(ctx.clone());
        let mut app = Self {
            state: AppState::new(),
            bridge,
            window_icon_theme: None,
            next_update_check: std::time::Instant::now()
                + std::time::Duration::from_secs(4 * 60 * 60),
        };
        if let Some(path) = initial_workspace {
            match Workspace::load(&path) {
                Ok(ws) => {
                    app.state.workspace = Some(ws);
                    app.on_workspace_opened(&ctx);
                    crate::dialogs::welcome::remember_recent(&path);
                }
                Err(e) => {
                    if path.join("project.json").exists() {
                        app.state.assets.load(path.clone());
                        match history::open_store(&path) {
                            Ok(store) => app.state.history_store = Some(store),
                            Err(error) => {
                                app.state.log.error("history", error.clone());
                                app.state.status = Some(StatusMessage::error(error));
                            }
                        }
                        if let Err(error) = app.bridge.send(Cmd::LoadCookies {
                            path: cookies::cookies_path(&path),
                        }) {
                            app.state.log.error("bridge", error);
                        }
                        app.state.show_assets = true;
                        app.state.show_collections = false;
                        app.state.show_environment = false;
                        app.state.dialogs.v1_editor.open_new(path, None);
                    } else {
                        app.state.status =
                            Some(StatusMessage::error(format!("{}: {e}", path.display())));
                    }
                }
            }
        }
        // Dev convenience: FORGE_OPEN=<workspace-relative request id> opens
        // that request in a tab on startup (used for headless screenshots).
        if let Ok(rel) = std::env::var("FORGE_OPEN") {
            app.open_request_tab(&rel);
        }
        if std::env::var("FORGE_SEND").is_ok() {
            request_editor::send_active(&mut app.state, &app.bridge);
        }
        app.state.dialogs.update.check(&app.bridge, false);
        app.state.dialogs.license.revalidate_on_start(&app.bridge);
        app
    }

    fn sync_window_icon(&mut self, ctx: &egui::Context) {
        let theme = ctx.system_theme().unwrap_or(egui::Theme::Dark);
        if self.window_icon_theme == Some(theme) {
            return;
        }
        if let Ok(icon) = eframe::icon_data::from_png_bytes(window_icon_png(theme)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(std::sync::Arc::new(icon))));
        }
        self.window_icon_theme = Some(theme);
    }

    /// Side effects that belong to "a workspace just became `self.state.workspace`":
    /// open its history store, ask the bridge to load its persisted cookie
    /// jar, and restore the last saved UI snapshot (open tabs, active
    /// environment/theme, visible tool windows).
    fn on_workspace_opened(&mut self, ctx: &egui::Context) {
        let Some(root) = self.state.workspace.as_ref().map(|w| w.root.clone()) else {
            return;
        };
        self.state
            .log
            .info("workspace", format!("Opened workspace {}", root.display()));
        match history::open_store(&root) {
            Ok(store) => {
                self.state.history_store = Some(store);
                self.state.history_ui.loaded = false;
            }
            Err(error) => {
                self.state.history_store = None;
                self.state.log.error("history", error.clone());
                self.state.status = Some(StatusMessage::error(error));
            }
        }
        if let Err(error) = self.bridge.send(Cmd::LoadCookies {
            path: cookies::cookies_path(&root),
        }) {
            self.state.log.error("bridge", error.clone());
            self.state.status = Some(StatusMessage::error(error));
        }
        if root.join("project.json").exists() {
            self.state.assets.load(root.clone());
        }
        if let Some(snapshot) = local::load(&root) {
            local::apply(&mut self.state, snapshot);
            self.state.theme.apply(ctx);
            crate::dialogs::settings::apply_typography(ctx, &self.state);
        } else if root.join("project.json").exists() {
            self.state.show_assets = true;
            self.state.show_collections = false;
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
        self.state.assets = Default::default();
        self.on_workspace_opened(ctx);
    }

    /// Record one finished request execution to the workspace's history
    /// store, if it has one open.
    fn record_history(&mut self, outcome: &RequestOutcome) {
        let Some(store) = self.state.history_store.as_ref() else {
            return;
        };
        let entry = history::new_entry_from_outcome(
            self.state.workspace.as_ref(),
            outcome,
            self.state.active_env.clone(),
        );
        if let Err(error) = store.record(entry) {
            let error = format!("failed to record history: {error}");
            self.state.log.error("history", error.clone());
            self.state.status = Some(StatusMessage::error(error));
        }
    }

    fn record_v1_history(&mut self, output: &crate::bridge::V1RunOutput) {
        let Some(store) = self.state.history_store.as_ref() else {
            return;
        };
        for item in &output.items {
            let entry = history::record_from_v1(item, self.state.active_env.clone());
            if let Err(error) = store.record_raw(entry) {
                let error = format!("failed to record reqv1 history: {error}");
                self.state.log.error("history", error.clone());
                self.state.status = Some(StatusMessage::error(error));
            }
        }
        self.state.history_ui.loaded = false;
    }

    /// Keep the parsed OpenAPI spec in sync with the workspace's
    /// `settings.openapi_url`: any change (workspace switch, settings save)
    /// triggers exactly one refetch through the bridge.
    fn sync_openapi(&mut self) {
        let root = self
            .state
            .workspace
            .as_ref()
            .map(|workspace| workspace.root.clone())
            .or_else(|| self.state.assets.project_root());
        let scoped = root.as_deref().and_then(|root| {
            self.state
                .dialogs
                .v1_editor
                .active_file()
                .and_then(|file| {
                    forge_core::reqv1::effective_openapi(root, file)
                        .ok()
                        .flatten()
                })
                .map(|selection| selection.value)
        });
        let want = scoped.or_else(|| {
            self.state
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.meta.settings.openapi_url.clone())
        });
        if want != self.state.openapi_source {
            self.state.openapi = None;
            self.state.openapi_error = None;
            self.state.openapi_source = want.clone();
            if let (Some(source), Some(root)) = (want, root) {
                if let Err(error) = self.bridge.send(Cmd::FetchOpenApi { root, source }) {
                    self.state.log.error("bridge", error.clone());
                    self.state.openapi_error = Some(error.clone());
                    self.state.status = Some(StatusMessage::error(error));
                }
            }
        }
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
                Evt::Ws { conn_id, event } => {
                    console::handle_ws_event(&mut self.state, conn_id, event)
                }
                Evt::Sse { conn_id, event } => {
                    console::handle_sse_event(&mut self.state, conn_id, event)
                }
                Evt::Cookies(cookies) => self.state.cookies_ui.rows = cookies,
                Evt::Grpc { call_id, result } => {
                    self.state.dialogs.grpc_call.handle_result(call_id, result)
                }
                Evt::V1Run { run_id, result } => {
                    if let Ok(output) = &result {
                        self.record_v1_history(output);
                    }
                    self.state.dialogs.v1_editor.handle_result(run_id, result)
                }
                Evt::V1Preview { preview_id, result } => self
                    .state
                    .dialogs
                    .v1_editor
                    .handle_preview(preview_id, result),
                Evt::OpenApi { source, result } => {
                    // Ignore replies for a source that is no longer wanted.
                    if self.state.openapi_source.as_deref() == Some(source.as_str()) {
                        match result.and_then(|text| {
                            forge_core::openapi::parse_spec(&text).map_err(|e| e.to_string())
                        }) {
                            Ok(spec) => {
                                self.state.log.info(
                                    "openapi",
                                    format!(
                                        "Loaded spec {:?} ({} operations) from {source}",
                                        spec.title,
                                        spec.operations.len()
                                    ),
                                );
                                self.state.openapi = Some(spec);
                                self.state.openapi_error = None;
                            }
                            Err(e) => {
                                self.state.log.error("openapi", format!("{source}: {e}"));
                                self.state.openapi = None;
                                self.state.openapi_error = Some(e);
                            }
                        }
                    }
                }
                Evt::Advisor { advisor_id, result } => self
                    .state
                    .dialogs
                    .v1_editor
                    .handle_advisor(advisor_id, result),
                Evt::UpdateChecked { manual, result } => {
                    self.state.dialogs.update.handle_check(manual, result)
                }
                Evt::UpdateDownloaded(result) => self.state.dialogs.update.handle_download(result),
                Evt::LicenseValidated { manual, result } => {
                    self.state.dialogs.license.handle_validated(manual, result)
                }
                #[cfg(feature = "pro")]
                Evt::JiraIssue { key, result } => self.state.dialogs.jira.handle_issue(key, result),
                #[cfg(feature = "pro")]
                Evt::JiraCommented { key, result } => {
                    self.state.dialogs.jira.handle_commented(key, result)
                }
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
                    Err(e) => self
                        .state
                        .log
                        .error("run", format!("{}: {e}", outcome.name)),
                    Ok(res) => {
                        let failed = outcome.assertions.iter().filter(|a| !a.passed).count();
                        if failed > 0 {
                            self.state.log.warn(
                                "run",
                                format!(
                                    "{}: {} of {} assertions failed",
                                    outcome.name,
                                    failed,
                                    outcome.assertions.len()
                                ),
                            );
                        } else {
                            self.state.log.info(
                                "run",
                                format!(
                                    "{} → {} ({} ms)",
                                    outcome.name,
                                    res.status,
                                    res.timing.total.as_millis()
                                ),
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
            ui.label(
                egui::RichText::new(title.to_uppercase())
                    .size(13.0)
                    .strong()
                    .color(ui.visuals().weak_text_color()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(egui::RichText::new(icons::TRIANGLE_LEFT).size(13.0))
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
        if let Some(def) = self
            .state
            .workspace
            .as_ref()
            .and_then(|ws| ws.find_request(rel_id).map(|n| n.def.clone()))
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

    fn open_api_project_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if !path.join("project.json").exists() {
            self.state.status = Some(StatusMessage::error(format!(
                "{} does not contain project.json",
                path.display()
            )));
            return;
        }
        if let Some(old_root) = self
            .state
            .workspace
            .as_ref()
            .map(|workspace| workspace.root.clone())
        {
            local::save(&old_root, &self.state);
        }
        self.state.workspace = None;
        self.state.tabs.clear();
        self.state.active_tab = None;
        self.state.assets.load(path.clone());
        match history::open_store(&path) {
            Ok(store) => {
                self.state.history_store = Some(store);
                self.state.history_ui.loaded = false;
            }
            Err(error) => {
                self.state.history_store = None;
                self.state.status = Some(StatusMessage::error(error));
            }
        }
        if let Err(error) = self.bridge.send(Cmd::LoadCookies {
            path: cookies::cookies_path(&path),
        }) {
            self.state.log.error("bridge", error);
        }
        self.state.show_assets = true;
        self.state.show_collections = false;
        self.state.show_environment = false;
        let env = self.state.active_env.clone();
        self.state.dialogs.v1_editor.open_new(path, env);
    }

    fn run_workspace(&mut self) {
        let Some(ws) = self.state.workspace.clone() else {
            self.state.status = Some(StatusMessage::error("No workspace open"));
            return;
        };
        let options = RunOptions {
            environment: self.state.active_env.clone(),
            ..Default::default()
        };
        self.state.last_run = Some((RunScope::Workspace, options.clone()));
        let run_id = self.state.alloc_run_id();
        self.state.run_state = RunState {
            run_id: Some(run_id),
            total: 0,
            completed: 0,
        };
        self.state.run_log.start(run_id);
        if let Err(error) = self.bridge.send(Cmd::Run {
            run_id,
            workspace: Box::new(ws),
            scope: RunScope::Workspace,
            options,
        }) {
            self.clear_run(run_id);
            self.state.log.error("bridge", error.clone());
            self.state.status = Some(StatusMessage::error(error));
        }
    }

    /// Build a menu-item button labelled with an [`ActionId`]'s registered
    /// title, showing its keyboard shortcut (if any) on the trailing side.
    fn action_button(ctx: &egui::Context, id: ActionId) -> egui::Button<'static> {
        let title = keymap::ACTIONS
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.title)
            .unwrap_or("");
        let mut button = egui::Button::new(title);
        if let Some(shortcut) = keymap::shortcut_for(id) {
            button = button.shortcut_text(ctx.format_shortcut(&shortcut));
        }
        button
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        // Relay menu bar: flat text menu items (dim, no resting fill; hover
        // brings a soft rounded fill), so kill the default button chrome for
        // everything inside this bar. The env pill re-adds its own box.
        let dim = self.state.theme.dim_color();
        {
            let v = &mut ui.style_mut().visuals;
            v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            v.widgets.open.weak_bg_fill = v.widgets.hovered.weak_bg_fill;
            // Menu items rest dimmed (the theme's override_text_color would
            // otherwise force them to full text color).
            v.override_text_color = Some(dim);
        }
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.menu_button("File", |ui| {
                if ui
                    .button("New Project...")
                    .on_hover_text("Create a ready-to-use ApiWright workspace")
                    .clicked()
                {
                    self.new_workspace_dialog();
                    ui.close();
                }
                if ui
                    .add(Self::action_button(ui.ctx(), ActionId::OpenWorkspace))
                    .on_hover_text("Open an existing ApiWright workspace")
                    .clicked()
                {
                    self.open_workspace_dialog();
                    ui.close();
                }
                if ui
                    .button("Open Standalone API Project...")
                    .on_hover_text("Open a folder that contains project.json")
                    .clicked()
                {
                    self.open_api_project_dialog();
                    ui.close();
                }
                ui.separator();
                let has_active = self.state.active_tab.is_some();
                if ui
                    .add_enabled(has_active, Self::action_button(ui.ctx(), ActionId::Save))
                    .on_hover_text("Save the active request")
                    .clicked()
                {
                    if let Some(idx) = self.state.active_tab {
                        save_tab(&mut self.state, idx);
                    }
                    ui.close();
                }
                if ui
                    .add(Self::action_button(ui.ctx(), ActionId::SaveAll))
                    .on_hover_text("Save every modified request")
                    .clicked()
                {
                    save_all(&mut self.state);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add(Self::action_button(ui.ctx(), ActionId::ImportCurl))
                    .on_hover_text("Create a request from a curl command")
                    .clicked()
                {
                    self.state.dialogs.curl_import.open();
                    ui.close();
                }
                if ui
                    .button("Import OpenAPI...")
                    .on_hover_text("Generate requests from an OpenAPI document")
                    .clicked()
                {
                    self.state.dialogs.openapi_import.open();
                    ui.close();
                }
                if ui
                    .button("Import Postman...")
                    .on_hover_text("Import a Postman collection")
                    .clicked()
                {
                    self.state.dialogs.postman_import.open();
                    ui.close();
                }
                if ui
                    .button("Import Bruno...")
                    .on_hover_text("Import a Bruno collection")
                    .clicked()
                {
                    self.state.dialogs.bruno_import.open();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add(Self::action_button(ui.ctx(), ActionId::OpenSettings))
                    .on_hover_text("Configure editor, view and project defaults")
                    .clicked()
                {
                    self.state.dialogs.settings.open = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").on_hover_text("Close ApiWright").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            })
            .response
            .on_hover_text("Project, file, import and application actions");
            ui.menu_button("Run", |ui| {
                let can_send = self.state.active_tab.is_some();
                if ui
                    .add_enabled(can_send, Self::action_button(ui.ctx(), ActionId::Send))
                    .on_hover_text("Execute the active request")
                    .clicked()
                {
                    request_editor::send_active(&mut self.state, &self.bridge);
                    ui.close();
                }
                let can_run = self.state.workspace.is_some();
                if ui
                    .add_enabled(can_run, egui::Button::new("Run Collection"))
                    .on_hover_text("Execute every request in the open legacy workspace")
                    .clicked()
                {
                    self.run_workspace();
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("gRPC Call...")
                    .on_hover_text("Open the gRPC request runner")
                    .clicked()
                {
                    self.state.dialogs.grpc_call.open();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(can_run, egui::Button::new("Coverage report…"))
                    .on_hover_text(
                        "Ticket → OpenAPI → coverage with runtime and flaky analysis (Pro)",
                    )
                    .clicked()
                {
                    ui.close();
                    #[cfg(feature = "pro")]
                    if self.state.dialogs.license.pro_features() {
                        crate::dialogs::report::open_dialog(&mut self.state);
                    } else {
                        self.state.status = Some(StatusMessage::info(
                            "The coverage report is a ApiWright Pro feature — start the free 60-day commercial trial or activate a license under Help → License & Billing.",
                        ));
                        self.state.dialogs.license.open_dialog();
                    }
                    #[cfg(not(feature = "pro"))]
                    {
                        self.state.status = Some(StatusMessage::info(
                            "The coverage report ships with ApiWright Pro builds — see Help → License & Billing.",
                        ));
                        self.state.dialogs.license.open_dialog();
                    }
                }
            })
            .response
            .on_hover_text("Execute requests, collections and gRPC calls");
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.state.show_collections, "Collections")
                    .on_hover_text("Show or hide the legacy collection explorer");
                ui.checkbox(&mut self.state.show_assets, "Project")
                    .on_hover_text("Show or hide the file-based project explorer");
                ui.checkbox(&mut self.state.show_environment, "Environment")
                    .on_hover_text("Show or hide legacy environment variables");
                ui.checkbox(&mut self.state.show_activity_bar, "Activity bar")
                    .on_hover_text("Show or hide the left tool-window icons");
                ui.checkbox(&mut self.state.show_bottom_bar, "Bottom tool bar")
                    .on_hover_text("Show or hide the bottom tool switcher");
                ui.checkbox(&mut self.state.show_status_bar, "Status bar")
                    .on_hover_text("Show or hide Git, timing and version status");
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    for kind in ThemeKind::ALL {
                        if ui
                            .selectable_label(self.state.theme == kind, kind.label())
                            .on_hover_text(format!("Switch to the {} theme", kind.label()))
                            .clicked()
                        {
                            self.state.theme = kind;
                            kind.apply(ui.ctx());
                            crate::dialogs::settings::apply_typography(ui.ctx(), &self.state);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Choose the application color theme");
                ui.separator();
                if ui
                    .add(Self::action_button(ui.ctx(), ActionId::ToggleZen))
                    .on_hover_text("Hide all chrome for a distraction-free editor")
                    .clicked()
                {
                    self.dispatch_action(ActionId::ToggleZen);
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Manage Environments...")
                    .on_hover_text("Create, edit or remove environments")
                    .clicked()
                {
                    let preferred = self.state.active_env.clone();
                    self.state.dialogs.env_editor.open(preferred);
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("User tour…")
                    .on_hover_text("Walk through the main ApiWright workflow")
                    .clicked()
                {
                    self.state.dialogs.tour.start();
                    ui.close();
                }
            })
            .response
            .on_hover_text("Control panels, appearance, focus mode and onboarding");
            ui.menu_button("Help", |ui| {
                if ui
                    .add_enabled(
                        !self.state.dialogs.update.checking,
                        egui::Button::new("Check for updates…"),
                    )
                    .on_hover_text("Check the configured release source for a newer ApiWright build")
                    .clicked()
                {
                    self.state.dialogs.update.check(&self.bridge, true);
                    ui.close();
                }
                if ui
                    .button("License & Billing…")
                    .on_hover_text("Current plan, pricing and license activation")
                    .clicked()
                {
                    self.state.dialogs.license.open_dialog();
                    ui.close();
                }
                if ui
                    .button("About ApiWright")
                    .on_hover_text("Show version and application information")
                    .clicked()
                {
                    self.state.dialogs.about_open = true;
                    ui.close();
                }
            })
            .response
            .on_hover_text("Updates and application information");

            // Right cluster (laid out right-to-left, so first added = rightmost).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(icons::ZEN).on_hover_text("Zen mode").clicked() {
                    self.dispatch_action(ActionId::ToggleZen);
                }
                ui.add_space(2.0);

                if self.state.workspace.is_some() {
                    self.env_pill(ui);
                }
            });
        });
    }

    /// The environment switcher pill in the top bar's right cluster: a
    /// rounded elevated button showing a status dot + the active environment,
    /// opening a menu to switch or manage environments.
    fn env_pill(&mut self, ui: &mut egui::Ui) {
        // Restore the boxed look the flat menu-bar scope stripped.
        {
            let elev = ui.visuals().widgets.hovered.bg_fill;
            let v = &mut ui.style_mut().visuals;
            v.widgets.inactive.weak_bg_fill = elev;
            v.override_text_color = None;
        }
        let env_names: Vec<String> = self
            .state
            .workspace
            .as_ref()
            .map(|w| w.environments.iter().map(|e| e.env.name.clone()).collect())
            .unwrap_or_default();
        let current = self
            .state
            .active_env
            .clone()
            .unwrap_or_else(|| "Automatic".to_string());
        let dot = if self.state.active_env.is_some() {
            "\u{25CF}"
        } else {
            "\u{25CB}"
        };
        ui.menu_button(format!("{dot} {current} \u{25BE}"), |ui| {
            ui.label(egui::RichText::new("ENVIRONMENTS").weak().small());
            if ui
                .selectable_label(self.state.active_env.is_none(), "Automatic (properties)")
                .on_hover_text("Use the environment inherited from request and folder properties")
                .clicked()
            {
                self.state.active_env = None;
                ui.close();
            }
            for name in env_names {
                let is_sel = self.state.active_env.as_deref() == Some(name.as_str());
                if ui
                    .selectable_label(is_sel, &name)
                    .on_hover_text(format!("Use the {name} environment"))
                    .clicked()
                {
                    self.state.active_env = Some(name);
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .button("Manage environments\u{2026}")
                .on_hover_text("Create, edit or remove environments")
                .clicked()
            {
                let preferred = self.state.active_env.clone();
                self.state.dialogs.env_editor.open(preferred);
                ui.close();
            }
        })
        .response
        .on_hover_text("Select the request environment");
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut close_idx: Option<usize> = None;
        let mut select_idx: Option<usize> = None;
        egui::ScrollArea::horizontal()
            .id_salt("tab-bar-scroll")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let accent = self.state.theme.accent_color();
                    for (i, tab) in self.state.tabs.iter().enumerate() {
                        let is_active = self.state.active_tab == Some(i);
                        // Relay tab look: the active tab is filled with the
                        // editor background (visually joining the tab to the
                        // editor below) plus a 2px accent underline.
                        let frame = egui::Frame::NONE
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .fill(if is_active {
                                self.state.theme.editor_bg()
                            } else {
                                egui::Color32::TRANSPARENT
                            });
                        let resp = frame
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // The badge+title group is the click target for
                                    // selecting the tab; the × button is a separate
                                    // widget so its click is never stolen by a
                                    // full-tab overlay (that was closing-bug #1).
                                    let body = ui
                                        .horizontal(|ui| {
                                            crate::widgets::method_badge::method_badge(
                                                ui,
                                                tab.def.method,
                                            );
                                            let title = if tab.dirty {
                                                format!("{} {}", tab.title(), icons::DIRTY)
                                            } else {
                                                tab.title().to_string()
                                            };
                                            ui.label(title);
                                        })
                                        .response;
                                    let click = ui.interact(
                                        body.rect,
                                        ui.id().with(("tab-body", i)),
                                        egui::Sense::click(),
                                    );
                                    if click.clicked() {
                                        select_idx = Some(i);
                                    }
                                    if click.middle_clicked() {
                                        close_idx = Some(i);
                                    }
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
                    }
                });
            });
        if let Some(i) = select_idx {
            if self.state.auto_save {
                if let Some(active) = self.state.active_tab {
                    if active != i && self.state.tabs.get(active).is_some_and(|tab| tab.dirty) {
                        save_tab(&mut self.state, active);
                    }
                }
            }
            self.state.active_tab = Some(i);
        }
        if let Some(i) = close_idx {
            if self.state.auto_save && self.state.tabs.get(i).is_some_and(|tab| tab.dirty) {
                save_tab(&mut self.state, i);
            }
            self.state.close_tab(i);
        }
    }

    /// Relay-style bottom console tab strip: the always-visible bottom-tool
    /// switcher. Selecting a tab opens its panel; clicking the active tab
    /// hides it.
    fn bottom_tool_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for (tool, icon) in [
                (BottomTool::Run, icons::RUN),
                (BottomTool::Problems, icons::PROBLEMS),
                (BottomTool::Terminal, icons::TERMINAL),
                (BottomTool::History, icons::HISTORY),
            ] {
                let active = self.state.bottom_tool == Some(tool);
                if ui
                    .selectable_label(active, format!("{icon}  {}", tool.label()))
                    .on_hover_text(format!("Open or hide the {} tool window", tool.label()))
                    .clicked()
                {
                    self.state.bottom_tool = if active { None } else { Some(tool) };
                }
            }
            ui.menu_button(format!("{}  More", icons::ELLIPSIS), |ui| {
                for (tool, icon) in [
                    (BottomTool::Log, icons::LOG),
                    (BottomTool::Console, icons::CONSOLE),
                    (BottomTool::Cookies, icons::COOKIES),
                    (BottomTool::Variables, icons::ENVIRONMENT),
                ] {
                    if ui
                        .button(format!("{icon}  {}", tool.label()))
                        .on_hover_text(format!("Open the {} tool window", tool.label()))
                        .clicked()
                    {
                        self.state.bottom_tool = Some(tool);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Show additional tool windows");
            if self.state.bottom_tool.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(icons::COLLAPSE)
                        .on_hover_text("Collapse the active bottom tool window")
                        .clicked()
                    {
                        self.state.bottom_tool = None;
                    }
                });
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let root = self
            .state
            .workspace
            .as_ref()
            .map(|workspace| workspace.root.clone())
            .or_else(|| self.state.assets.project_root());
        if let Some(root) = root.as_deref() {
            self.state.git.refresh(root, false);
        }
        let dim = self.state.theme.dim_color();
        let mono = |t: String| egui::RichText::new(t).monospace().size(13.0).color(dim);
        ui.horizontal(|ui| {
            let left = self
                .state
                .git
                .status
                .as_ref()
                .and_then(|s| s.branch.clone())
                .or_else(|| self.state.workspace.as_ref().map(|w| w.meta.name.clone()))
                .or_else(|| self.state.assets.project_name())
                .unwrap_or_else(|| {
                    if self.state.dialogs.v1_editor.open {
                        "API project".to_string()
                    } else {
                        "No workspace".to_string()
                    }
                });
            if let Some(root) = root.clone().filter(|_| self.state.git.status.is_some()) {
                ui.menu_button(mono(format!("{}  {left}", icons::BRANCH)), |ui| {
                    match crate::git::branches(&root) {
                        Ok(branches) => {
                            for branch in branches {
                                if ui.selectable_label(branch == left, &branch).clicked() {
                                    match crate::git::switch_branch(&root, &branch) {
                                        Ok(()) => {
                                            self.state.git.refresh(&root, true);
                                            self.state.status = Some(StatusMessage::info(format!(
                                                "Switched to {branch}"
                                            )));
                                        }
                                        Err(error) => {
                                            self.state.status = Some(StatusMessage::error(error));
                                        }
                                    }
                                    ui.close();
                                }
                            }
                        }
                        Err(error) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                    }
                    ui.separator();
                    if ui.button("New worktree…").clicked() {
                        self.open_worktree_dialog(&root);
                        ui.close();
                    }
                });
            } else {
                ui.label(mono(format!("{}  {left}", icons::BRANCH)));
            }

            // Active environment (read-only here; switch via the top-bar pill).
            if self.state.workspace.is_some() {
                ui.add_space(6.0);
                let env_active = self.state.active_env.is_some();
                status_dot(
                    ui,
                    if env_active {
                        self.state.theme.accent_color()
                    } else {
                        dim
                    },
                );
                ui.label(mono(
                    self.state
                        .active_env
                        .clone()
                        .unwrap_or_else(|| "No Environment".to_string()),
                ));
            }

            ui.add_space(6.0);
            if self.state.run_state.is_running() {
                ui.spinner();
                ui.label(mono(format!(
                    "Running {}/{}",
                    self.state.run_state.completed, self.state.run_state.total
                )));
            } else {
                ui.label(mono("Ready".into()));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(mono(format!("v{}", env!("CARGO_PKG_VERSION"))));
                if let Some(milliseconds) = self.current_execution_ms() {
                    ui.label(mono(format!("{milliseconds} ms")));
                }
            });
        });
    }

    fn current_execution_ms(&self) -> Option<u128> {
        if self.state.dialogs.v1_editor.open {
            return self
                .state
                .dialogs
                .v1_editor
                .last_execution_ms()
                .map(u128::from);
        }
        self.state
            .active_tab_ref()?
            .response
            .as_ref()?
            .result
            .as_ref()
            .ok()
            .map(|response| response.timing.total.as_millis())
    }

    fn open_worktree_dialog(&mut self, root: &std::path::Path) {
        let project_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project");
        self.state.git.worktree_branch = "feature".to_string();
        self.state.git.worktree_path = root
            .parent()
            .unwrap_or(root)
            .join(format!("{project_name}-feature"))
            .display()
            .to_string();
        self.state.git.worktree_open = true;
    }

    fn worktree_dialog(&mut self, ctx: &egui::Context) {
        if !self.state.git.worktree_open {
            return;
        }
        let Some(root) = self
            .state
            .workspace
            .as_ref()
            .map(|workspace| workspace.root.clone())
            .or_else(|| self.state.assets.project_root())
        else {
            self.state.git.worktree_open = false;
            return;
        };
        let mut open = true;
        let mut create = false;
        egui::Window::new("New worktree")
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("worktree-form")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Branch");
                        ui.text_edit_singleline(&mut self.state.git.worktree_branch);
                        ui.end_row();
                        ui.label("Path");
                        ui.text_edit_singleline(&mut self.state.git.worktree_path);
                        ui.end_row();
                    });
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    create = ui.button("Create").clicked();
                });
            });
        if create {
            let path = PathBuf::from(self.state.git.worktree_path.trim());
            match crate::git::create_worktree(&root, &path, &self.state.git.worktree_branch) {
                Ok(()) => {
                    self.state.status = Some(StatusMessage::info(format!(
                        "Created worktree {}",
                        path.display()
                    )));
                    open = false;
                }
                Err(error) => self.state.status = Some(StatusMessage::error(error)),
            }
        }
        self.state.git.worktree_open = open;
    }

    fn update_zen_reveals(&mut self, ctx: &egui::Context) {
        if !self.state.zen_mode {
            self.state.zen_left_revealed = false;
            self.state.zen_right_revealed = false;
            self.state.zen_bottom_revealed = false;
            return;
        }
        let Some(pointer) = ctx.pointer_hover_pos() else {
            return;
        };
        let rect = ctx.content_rect();
        if pointer.x <= rect.left() + 12.0 {
            self.state.zen_left_revealed = true;
        } else if self.state.zen_left_revealed && pointer.x > rect.left() + 500.0 {
            self.state.zen_left_revealed = false;
        }
        if pointer.x >= rect.right() - 12.0 {
            self.state.zen_right_revealed = true;
        } else if self.state.zen_right_revealed && pointer.x < rect.right() - 500.0 {
            self.state.zen_right_revealed = false;
        }
        if pointer.y >= rect.bottom() - 12.0 {
            self.state.zen_bottom_revealed = true;
        } else if self.state.zen_bottom_revealed && pointer.y < rect.bottom() - 420.0 {
            self.state.zen_bottom_revealed = false;
        }
    }

    fn zen_exit_button(&mut self, ctx: &egui::Context) {
        if !self.state.zen_mode {
            return;
        }
        let reveal = ctx.pointer_hover_pos().is_some_and(|pointer| {
            let rect = ctx.content_rect();
            pointer.x >= rect.right() - 72.0 && pointer.y <= rect.top() + 56.0
        });
        if reveal {
            egui::Area::new(egui::Id::new("zen-exit"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    if ui.button(format!("{}  Exit Zen", icons::ZEN)).clicked() {
                        self.dispatch_action(ActionId::ToggleZen);
                    }
                });
        }
    }

    fn toast(&mut self, ui: &mut egui::Ui) {
        let Some(status) = &self.state.status else {
            return;
        };
        if status.expired() {
            self.state.status = None;
            return;
        }
        let color = if status.is_error {
            self.state.theme.error_color()
        } else {
            self.state.theme.ok_color()
        };
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
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
}

impl eframe::App for ForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.sync_window_icon(ui.ctx());
        if std::time::Instant::now() >= self.next_update_check {
            self.state.dialogs.update.check(&self.bridge, false);
            self.next_update_check =
                std::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60);
        }
        self.drain_bridge_events();
        if let Some(ws) = self.state.pending_workspace.take() {
            self.switch_workspace(ws, ui.ctx());
        }
        self.sync_openapi();
        self.update_zen_reveals(ui.ctx());

        crate::dialogs::handle_global_shortcuts(ui.ctx(), &mut self.state);
        if let Some(action) = keymap::dispatch(ui.ctx()) {
            self.dispatch_action(action);
        }
        let api_project = self.state.assets.is_loaded();
        let has_project = self.state.workspace.is_some() || api_project;

        if !self.state.zen_mode {
            egui::Panel::top("menu-bar")
                .resizable(false)
                .show(ui, |ui| {
                    self.menu_bar(ui);
                });
        }

        if has_project
            && self.state.show_status_bar
            && (!self.state.zen_mode || self.state.zen_bottom_revealed)
        {
            egui::Panel::bottom("status-bar")
                .exact_size(26.0)
                .resizable(false)
                .show(ui, |ui| {
                    self.status_bar(ui);
                });
        }

        if has_project
            && self.state.show_activity_bar
            && (!self.state.zen_mode || self.state.zen_left_revealed)
        {
            egui::Panel::left("activity-rail")
                .exact_size(50.0)
                .resizable(false)
                .show(ui, |ui| {
                    let accent = self.state.theme.accent_color();
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        if !api_project
                            && rail_button(
                                ui,
                                self.state.show_collections,
                                icons::COLLECTIONS,
                                "Collections",
                                accent,
                            )
                        {
                            self.state.show_collections = !self.state.show_collections;
                        }
                        if !api_project {
                            ui.add_space(4.0);
                        }
                        if rail_button(ui, self.state.show_assets, icons::ASSETS, "Project", accent)
                        {
                            self.state.show_assets = !self.state.show_assets;
                        }
                        ui.add_space(4.0);
                        let hist = self.state.bottom_tool == Some(BottomTool::History);
                        if rail_button(ui, hist, icons::HISTORY, "History", accent) {
                            self.state.bottom_tool = if hist {
                                None
                            } else {
                                Some(BottomTool::History)
                            };
                        }
                        ui.add_space(4.0);
                        let run = self.state.bottom_tool == Some(BottomTool::Run);
                        if rail_button(ui, run, icons::PULSE, "Run results", accent) {
                            self.state.bottom_tool = if run { None } else { Some(BottomTool::Run) };
                        }
                        if !api_project {
                            ui.add_space(4.0);
                            if rail_button(
                                ui,
                                self.state.show_environment,
                                icons::ENVIRONMENT,
                                "Environment",
                                accent,
                            ) {
                                self.state.show_environment = !self.state.show_environment;
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if rail_button(ui, false, icons::GEAR, "Settings", accent) {
                            self.state.dialogs.settings.open = true;
                        }
                    });
                });
        }

        if has_project
            && self.state.show_collections
            && !api_project
            && (!self.state.zen_mode || self.state.zen_left_revealed)
        {
            egui::Panel::left("left-panel")
                .exact_size(280.0)
                .resizable(true)
                .size_range(180.0..=520.0)
                .show(ui, |ui| {
                    if Self::tool_window_header(ui, "Collections") {
                        self.state.show_collections = false;
                    }
                    collections::show(ui, &mut self.state, &self.bridge);
                });
        }

        if has_project && (!self.state.zen_mode || self.state.zen_left_revealed) {
            let mut show_assets = self.state.show_assets;
            let mut collapse_requested = false;
            egui::Panel::left("assets-panel")
                .default_size(320.0)
                .resizable(true)
                .size_range(260.0..=460.0)
                .show_collapsible(ui, &mut show_assets, |ui| {
                    if Self::tool_window_header(ui, "Project") {
                        collapse_requested = true;
                    }
                    assets::show(ui, &mut self.state, &self.bridge);
                });
            self.state.show_assets = show_assets && !collapse_requested;
        }

        if has_project
            && self.state.show_environment
            && !api_project
            && (!self.state.zen_mode || self.state.zen_right_revealed)
        {
            egui::Panel::right("right-panel")
                .exact_size(260.0)
                .resizable(true)
                .size_range(180.0..=480.0)
                .show(ui, |ui| {
                    if Self::tool_window_header(ui, "Environment") {
                        self.state.show_environment = false;
                    }
                    environment_panel(ui, &mut self.state);
                });
        }

        if has_project
            && self.state.show_bottom_bar
            && (!self.state.zen_mode || self.state.zen_bottom_revealed)
        {
            egui::Panel::bottom("tool-tabs")
                .exact_size(34.0)
                .resizable(false)
                .show(ui, |ui| {
                    self.bottom_tool_tabs(ui);
                });
        }

        if has_project && (!self.state.zen_mode || self.state.zen_bottom_revealed) {
            if let Some(tool) = self.state.bottom_tool {
                egui::Panel::bottom("bottom-tool-panel")
                    .default_size(240.0)
                    .resizable(true)
                    .size_range(120.0..=560.0)
                    .show(ui, |ui| {
                        ui.set_min_height(ui.available_height());
                        match tool {
                            BottomTool::Run => {
                                test_results::show(ui, &mut self.state, &self.bridge)
                            }
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
                            BottomTool::Variables => {
                                if let Some(rel_id) = variables::show(ui, &mut self.state) {
                                    self.open_request_tab(&rel_id);
                                }
                            }
                        }
                    });
            }
        }

        let editor_bg = self.state.theme.editor_bg();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(editor_bg))
            .show(ui, |ui| {
                if self.state.dialogs.v1_editor.open {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 8,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            crate::dialogs::v1_editor::show(ui, &mut self.state, &self.bridge);
                        });
                    return;
                }
                if !has_project {
                    crate::dialogs::welcome::show(ui, &mut self.state);
                    return;
                }
                if api_project {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("No request open").size(20.0).strong());
                            ui.label(
                                egui::RichText::new(
                                    "Select a request from the Project panel or create a new one.",
                                )
                                .weak(),
                            );
                        });
                    });
                    return;
                }
                // Tab strip sits on the lighter panel bg (chrome), full-width, so
                // it reads as a strip above the darker editor content — the
                // JetBrains/Relay contrast the flat single-bg look was missing.
                let panel_bg = ui.visuals().panel_fill;
                egui::Frame::NONE
                    .fill(panel_bg)
                    .inner_margin(egui::Margin::symmetric(2, 0))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.tab_bar(ui);
                    });
                ui.separator();
                if self.state.active_tab.is_some() {
                    // Relay-consistent 12px horizontal gutter around the editor
                    // content (the tab strip above stays full-bleed).
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 2,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            request_editor::show(ui, &mut self.state, &self.bridge);
                        });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.weak("Open a request from the Collections panel to get started.");
                    });
                }
            });

        crate::dialogs::show(ui.ctx(), &mut self.state, &self.bridge);
        self.worktree_dialog(ui.ctx());
        self.zen_exit_button(ui.ctx());
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

/// One activity-rail button: a 34×34 icon target that lights up on hover and
/// gets a New-UI accent selection bar down its left edge when active. Returns
/// `true` on click.
fn rail_button(
    ui: &mut egui::Ui,
    active: bool,
    icon: &str,
    tip: &str,
    accent: egui::Color32,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
    let resp = resp.on_hover_text(tip);
    if active {
        ui.painter()
            .rect_filled(rect, 8u8, ui.visuals().widgets.active.bg_fill);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 8u8, ui.visuals().widgets.hovered.bg_fill);
    }
    if active {
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, rect.top() + 7.0),
            egui::pos2(rect.left() + 4.0, rect.bottom() - 7.0),
        );
        ui.painter().rect_filled(bar, 2u8, accent);
    }
    let color = if active {
        egui::Color32::WHITE
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(18.0),
        color,
    );
    resp.clicked()
}

/// A small filled status dot (environment indicator in the status bar).
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
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
        ui.weak("Automatic environment selection.");
        ui.weak("Folder and request properties decide at run time.");
        return;
    };
    let Some(loaded) = workspace.environment(env_name) else {
        ui.weak("Environment not found.");
        return;
    };
    egui::ScrollArea::vertical()
        .id_salt("app-sa-1")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("env-vars-grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Name");
                    ui.strong("Value");
                    ui.end_row();
                    for (name, var) in &loaded.env.variables {
                        ui.label(name);
                        if var.secret {
                            let has_value = loaded.secrets.contains_key(name);
                            ui.weak(if has_value {
                                "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
                            } else {
                                "(not set)"
                            });
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
    let Some(tab) = state.tabs.get_mut(idx) else {
        return;
    };
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

    #[test]
    fn themed_window_icons_are_distinct_valid_pngs() {
        let light = eframe::icon_data::from_png_bytes(window_icon_png(egui::Theme::Light)).unwrap();
        let dark = eframe::icon_data::from_png_bytes(window_icon_png(egui::Theme::Dark)).unwrap();
        assert_eq!((light.width, light.height), (256, 256));
        assert_eq!((dark.width, dark.height), (256, 256));
        assert_ne!(light.rgba, dark.rgba);
    }

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
        app.state.run_state = RunState {
            run_id: Some(2),
            total: 5,
            completed: 1,
        };

        app.handle_run_event(
            1,
            RunEvent::RequestFinished(Box::new(dummy_outcome("req-a"))),
        );
        assert_eq!(
            app.state.run_state.completed, 1,
            "a RequestFinished from a stale run_id must not bump the current run's completed count"
        );

        let summary = RunSummary {
            total: 1,
            passed: 1,
            failed: 0,
            skipped: 0,
            duration_ms: 5,
        };
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
        app.handle_run_event(
            2,
            RunEvent::RequestFinished(Box::new(dummy_outcome("req-b"))),
        );
        assert_eq!(app.state.run_state.completed, 2);
    }
}
