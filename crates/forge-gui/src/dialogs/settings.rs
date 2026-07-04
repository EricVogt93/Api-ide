//! IntelliJ-style Settings dialog (`Ctrl+Alt+S` / File → Settings...): a
//! left-hand category list plus per-category content on the right.
//!
//! Appearance (theme) and Editor (font size) apply immediately, matching the
//! View menu's own theme picker. HTTP workspace settings are edited on a
//! draft copy with explicit Save/Apply/Cancel semantics, since they're
//! persisted to `forge.json` rather than taking effect purely in memory.

use egui::{TextEdit, Ui, Window};
use forge_core::model::{ProxyConfig, WorkspaceSettings};

use crate::keymap;
use crate::state::{AppState, StatusMessage};
use crate::theme::ThemeKind;

/// Left-nav category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Category {
    #[default]
    Appearance,
    Http,
    Editor,
    Keymap,
}

impl Category {
    const ALL: [Category; 4] = [Category::Appearance, Category::Http, Category::Editor, Category::Keymap];

    fn label(&self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Http => "HTTP",
            Category::Editor => "Editor",
            Category::Keymap => "Keymap",
        }
    }
}

/// Draft copy of the HTTP workspace settings; only these are committed
/// explicitly (Save/Apply) rather than applied live.
#[derive(Debug, Clone)]
struct HttpDraft {
    timeout_ms: u64,
    follow_redirects: bool,
    max_redirects: u32,
    verify_tls: bool,
    proxy_enabled: bool,
    proxy_url: String,
    proxy_no_proxy: String,
    user_agent: String,
}

impl HttpDraft {
    fn from_settings(s: &WorkspaceSettings) -> Self {
        Self {
            timeout_ms: s.timeout_ms,
            follow_redirects: s.follow_redirects,
            max_redirects: s.max_redirects,
            verify_tls: s.verify_tls,
            proxy_enabled: s.proxy.is_some(),
            proxy_url: s.proxy.as_ref().map(|p| p.url.clone()).unwrap_or_default(),
            proxy_no_proxy: s.proxy.as_ref().map(|p| p.no_proxy.clone()).unwrap_or_default(),
            user_agent: s.user_agent.clone().unwrap_or_default(),
        }
    }

    fn to_settings(&self) -> WorkspaceSettings {
        WorkspaceSettings {
            timeout_ms: self.timeout_ms,
            follow_redirects: self.follow_redirects,
            max_redirects: self.max_redirects,
            verify_tls: self.verify_tls,
            proxy: if self.proxy_enabled {
                Some(ProxyConfig { url: self.proxy_url.clone(), no_proxy: self.proxy_no_proxy.clone() })
            } else {
                None
            },
            user_agent: if self.user_agent.is_empty() { None } else { Some(self.user_agent.clone()) },
        }
    }
}

/// Transient state of the Settings dialog, owned by [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct SettingsState {
    pub open: bool,
    category: Category,
    draft: Option<HttpDraft>,
}

/// Render the Settings window if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.settings.open {
        state.dialogs.settings.draft = None;
        return;
    }
    if state.dialogs.settings.draft.is_none() {
        let settings = state.workspace.as_ref().map(|w| w.meta.settings.clone()).unwrap_or_default();
        state.dialogs.settings.draft = Some(HttpDraft::from_settings(&settings));
    }

    let mut window_open = true;
    let mut save_clicked = false;
    let mut apply_clicked = false;
    let mut cancel_clicked = false;

    Window::new("Settings")
        .id(egui::Id::new("settings-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 420.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    for cat in Category::ALL {
                        if ui.selectable_label(state.dialogs.settings.category == cat, cat.label()).clicked() {
                            state.dialogs.settings.category = cat;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width(440.0);
                    ui.set_min_height(300.0);
                    match state.dialogs.settings.category {
                        Category::Appearance => appearance_tab(ui, state),
                        Category::Http => {
                            if let Some(draft) = state.dialogs.settings.draft.as_mut() {
                                http_tab(ui, draft);
                            }
                        }
                        Category::Editor => editor_tab(ui, state),
                        Category::Keymap => keymap_tab(ui),
                    }
                });
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
                if ui.button("Apply").clicked() {
                    apply_clicked = true;
                }
                if ui.button("Save").clicked() {
                    save_clicked = true;
                }
            });
        });

    if save_clicked || apply_clicked {
        if let Some(draft) = state.dialogs.settings.draft.clone() {
            save_http_settings(state, &draft);
        }
    }
    if save_clicked || cancel_clicked || !window_open {
        state.dialogs.settings.open = false;
        state.dialogs.settings.draft = None;
    }
}

fn save_http_settings(state: &mut AppState, draft: &HttpDraft) {
    let Some(workspace) = state.workspace.as_mut() else {
        state.status = Some(StatusMessage::error("No workspace open"));
        return;
    };
    workspace.meta.settings = draft.to_settings();
    match workspace.save_meta() {
        Ok(()) => state.status = Some(StatusMessage::info("Settings saved")),
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}

fn appearance_tab(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Appearance");
    ui.add_space(8.0);
    ui.label("Theme:");
    for kind in ThemeKind::ALL {
        if ui.radio(state.theme == kind, kind.label()).clicked() {
            state.theme = kind;
            kind.apply(ui.ctx());
        }
    }
}

fn editor_tab(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Editor");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("Monospace font size:");
        if ui.add(egui::Slider::new(&mut state.editor_font_size, 9.0..=24.0).suffix(" px")).changed() {
            apply_editor_font_size(ui.ctx(), state.editor_font_size);
        }
    });
}

/// Apply the monospace font size to every registered [`egui::TextStyle`]
/// that uses it, across both theme slots so it survives a theme switch.
pub fn apply_editor_font_size(ctx: &egui::Context, size: f32) {
    ctx.all_styles_mut(|style| {
        if let Some(id) = style.text_styles.get_mut(&egui::TextStyle::Monospace) {
            id.size = size;
        }
    });
}

fn http_tab(ui: &mut Ui, draft: &mut HttpDraft) {
    ui.heading("HTTP");
    ui.add_space(8.0);
    egui::Grid::new("http-settings-grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
        ui.label("Timeout (ms)");
        ui.add(egui::DragValue::new(&mut draft.timeout_ms).range(1..=600_000));
        ui.end_row();

        ui.label("Follow redirects");
        ui.checkbox(&mut draft.follow_redirects, "");
        ui.end_row();

        ui.label("Max redirects");
        ui.add(egui::DragValue::new(&mut draft.max_redirects).range(0..=50));
        ui.end_row();

        ui.label("Verify TLS certificates");
        ui.checkbox(&mut draft.verify_tls, "");
        ui.end_row();

        ui.label("User-Agent");
        ui.add(TextEdit::singleline(&mut draft.user_agent).hint_text("(default)"));
        ui.end_row();
    });

    ui.add_space(8.0);
    ui.checkbox(&mut draft.proxy_enabled, "Use a proxy");
    ui.add_enabled_ui(draft.proxy_enabled, |ui| {
        egui::Grid::new("http-proxy-grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
            ui.label("Proxy URL");
            ui.add(TextEdit::singleline(&mut draft.proxy_url).hint_text("http://127.0.0.1:8080"));
            ui.end_row();

            ui.label("No proxy for");
            ui.add(TextEdit::singleline(&mut draft.proxy_no_proxy).hint_text("comma-separated host suffixes"));
            ui.end_row();
        });
    });
}

fn keymap_tab(ui: &mut Ui) {
    ui.heading("Keymap");
    ui.add_space(8.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("keymap-grid").num_columns(2).striped(true).spacing([16.0, 4.0]).show(ui, |ui| {
            ui.strong("Action");
            ui.strong("Shortcut");
            ui.end_row();
            for action in keymap::ACTIONS {
                ui.label(action.title);
                match action.shortcut {
                    Some(shortcut) => ui.monospace(ui.ctx().format_shortcut(&shortcut)),
                    None => ui.weak("—"),
                };
                ui.end_row();
            }
        });
    });
}
