//! Jira in the GUI: the global connection config, the ticket-details
//! dialog (summary/status/assignee straight from Jira) and comment
//! posting. Fetching and commenting are Pro features — the gate sits at
//! the panel that opens this dialog.

use std::path::PathBuf;

use forge_pro::jira::{JiraConfig, JiraIssue};

use crate::bridge::{Bridge, Cmd};
use crate::state::AppState;

const JIRA_FILE: &str = "jira.json";

fn config_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("forge")
            .join(JIRA_FILE),
    )
}

/// Load the connection settings; missing or unreadable means unconfigured.
pub fn load_config() -> JiraConfig {
    config_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_config(config: &JiraConfig) -> Result<(), String> {
    let file = config_file().ok_or_else(|| "Cannot determine the config path.".to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create the config directory: {error}"))?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| format!("Cannot serialize the Jira settings: {error}"))?;
    std::fs::write(&file, text)
        .map_err(|error| format!("Cannot save the Jira settings: {error}"))?;
    // The file holds an API token: keep it owner-readable only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[derive(Default)]
pub struct JiraState {
    pub open: bool,
    key: String,
    loading: bool,
    posting: bool,
    issue: Option<JiraIssue>,
    comment: String,
    error: Option<String>,
    notice: Option<String>,
    /// Settings-tab draft, loaded once per app run.
    pub config: JiraConfig,
    config_loaded: bool,
}

impl JiraState {
    pub fn ensure_config(&mut self) {
        if !self.config_loaded {
            self.config = load_config();
            self.config_loaded = true;
        }
    }

    /// Open the dialog with the comment box prefilled (e.g. a coverage
    /// report section) and start fetching the ticket details.
    pub fn open_ticket_with_comment(&mut self, key: String, comment: String, bridge: &Bridge) {
        self.open_ticket(key, bridge);
        self.comment = comment;
    }

    /// Open the dialog for `key` and start fetching its details.
    pub fn open_ticket(&mut self, key: String, bridge: &Bridge) {
        self.open = true;
        if self.key != key {
            self.issue = None;
            self.comment.clear();
        }
        self.key = key;
        self.notice = None;
        self.fetch(bridge);
    }

    fn fetch(&mut self, bridge: &Bridge) {
        self.loading = true;
        self.error = None;
        if let Err(error) = bridge.send(Cmd::JiraFetchIssue {
            key: self.key.clone(),
        }) {
            self.loading = false;
            self.error = Some(error);
        }
    }

    pub fn handle_issue(&mut self, key: String, result: Result<JiraIssue, String>) {
        if key != self.key {
            return; // stale reply for a previously opened ticket
        }
        self.loading = false;
        match result {
            Ok(issue) => {
                self.issue = Some(issue);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn handle_commented(&mut self, key: String, result: Result<(), String>) {
        if key != self.key {
            return;
        }
        self.posting = false;
        match result {
            Ok(()) => {
                self.notice = Some(format!("Comment posted to {key}."));
                self.comment.clear();
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

enum DialogAction {
    None,
    Refresh,
    OpenUrl(String),
    PostComment,
}

pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    let dialog = &mut state.dialogs.jira;
    if !dialog.open {
        return;
    }
    let mut open = dialog.open;
    let mut action = DialogAction::None;
    let mut open_settings = false;
    egui::Window::new(format!("Jira — {}", dialog.key))
        .id(egui::Id::new("jira-dialog"))
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            if dialog.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Fetching from Jira…");
                });
            }
            if let Some(issue) = &dialog.issue {
                egui::Grid::new("jira-issue-grid")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Summary");
                        ui.label(&issue.summary);
                        ui.end_row();
                        ui.label("Status");
                        ui.strong(&issue.status);
                        ui.end_row();
                        ui.label("Type");
                        ui.label(&issue.issue_type);
                        ui.end_row();
                        ui.label("Assignee");
                        ui.label(issue.assignee.as_deref().unwrap_or("—"));
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Open in Jira").clicked() {
                        action = DialogAction::OpenUrl(issue.url.clone());
                    }
                    if ui
                        .add_enabled(!dialog.loading, egui::Button::new("Refresh"))
                        .clicked()
                    {
                        action = DialogAction::Refresh;
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.strong("Comment on this ticket");
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.comment)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text("e.g. ApiWright run: 12 passed, 0 failed"),
                );
                if ui
                    .add_enabled(
                        !dialog.posting && !dialog.comment.trim().is_empty(),
                        egui::Button::new(if dialog.posting {
                            "Posting…"
                        } else {
                            "Post comment"
                        }),
                    )
                    .clicked()
                {
                    action = DialogAction::PostComment;
                }
            } else if !dialog.loading && dialog.error.is_none() {
                ui.weak("No details loaded.");
            }

            if let Some(error) = &dialog.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
                if error.contains("not configured") && ui.button("Open Settings…").clicked() {
                    open_settings = true;
                }
            }
            if let Some(notice) = &dialog.notice {
                ui.add_space(4.0);
                ui.weak(notice);
            }
        });

    match action {
        DialogAction::None => {}
        DialogAction::Refresh => dialog.fetch(bridge),
        DialogAction::OpenUrl(url) => {
            if let Err(error) = open::that(&url) {
                dialog.error = Some(format!("Cannot open {url}: {error}"));
            }
        }
        DialogAction::PostComment => {
            dialog.posting = true;
            dialog.notice = None;
            dialog.error = None;
            if let Err(error) = bridge.send(Cmd::JiraAddComment {
                key: dialog.key.clone(),
                body: dialog.comment.clone(),
            }) {
                dialog.posting = false;
                dialog.error = Some(error);
            }
        }
    }
    dialog.open = open;
    if open_settings {
        state.dialogs.settings.open = true;
    }
}
