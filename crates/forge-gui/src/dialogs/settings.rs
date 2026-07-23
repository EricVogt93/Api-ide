//! IntelliJ-style Settings dialog (`Ctrl+Alt+S` / File → Settings...): a
//! left-hand category list plus per-category content on the right.
//!
//! Appearance (theme) and Editor (font size) apply immediately, matching the
//! View menu's own theme picker. HTTP workspace settings are edited on a
//! draft copy with explicit Save/Apply/Cancel semantics, since they're
//! persisted to `forge.json` rather than taking effect purely in memory.

use egui::{TextEdit, Ui, Window};
use forge_core::model::{ProxyConfig, TlsSettings, WorkspaceSettings};

use crate::keymap;
use crate::state::{AppState, StatusMessage, UiFont};
use crate::theme::ThemeKind;

/// Left-nav category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Category {
    #[default]
    Appearance,
    View,
    Http,
    Editor,
    #[cfg(feature = "pro")]
    Jira,
    Keymap,
}

impl Category {
    #[cfg(feature = "pro")]
    const ALL: [Category; 6] = [
        Category::Appearance,
        Category::View,
        Category::Http,
        Category::Editor,
        Category::Jira,
        Category::Keymap,
    ];
    #[cfg(not(feature = "pro"))]
    const ALL: [Category; 5] = [
        Category::Appearance,
        Category::View,
        Category::Http,
        Category::Editor,
        Category::Keymap,
    ];

    fn label(&self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::View => "View",
            Category::Http => "HTTP",
            Category::Editor => "Editor",
            #[cfg(feature = "pro")]
            Category::Jira => "Jira",
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
    tls_client_cert: String,
    tls_client_key: String,
    tls_ca_bundle: String,
    openapi_url: String,
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
            proxy_no_proxy: s
                .proxy
                .as_ref()
                .map(|p| p.no_proxy.clone())
                .unwrap_or_default(),
            user_agent: s.user_agent.clone().unwrap_or_default(),
            tls_client_cert: s
                .tls
                .as_ref()
                .and_then(|t| t.client_cert.clone())
                .unwrap_or_default(),
            tls_client_key: s
                .tls
                .as_ref()
                .and_then(|t| t.client_key.clone())
                .unwrap_or_default(),
            tls_ca_bundle: s
                .tls
                .as_ref()
                .and_then(|t| t.ca_bundle.clone())
                .unwrap_or_default(),
            openapi_url: s.openapi_url.clone().unwrap_or_default(),
        }
    }

    fn to_settings(&self) -> WorkspaceSettings {
        WorkspaceSettings {
            timeout_ms: self.timeout_ms,
            follow_redirects: self.follow_redirects,
            max_redirects: self.max_redirects,
            verify_tls: self.verify_tls,
            proxy: if self.proxy_enabled {
                Some(ProxyConfig {
                    url: self.proxy_url.clone(),
                    no_proxy: self.proxy_no_proxy.clone(),
                })
            } else {
                None
            },
            user_agent: if self.user_agent.is_empty() {
                None
            } else {
                Some(self.user_agent.clone())
            },
            tls: {
                let opt = |s: &str| {
                    let s = s.trim();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                };
                let tls = TlsSettings {
                    client_cert: opt(&self.tls_client_cert),
                    client_key: opt(&self.tls_client_key),
                    ca_bundle: opt(&self.tls_ca_bundle),
                };
                if tls.is_empty() {
                    None
                } else {
                    Some(tls)
                }
            },
            openapi_url: {
                let s = self.openapi_url.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            },
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
        let settings = state
            .workspace
            .as_ref()
            .map(|w| w.meta.settings.clone())
            .unwrap_or_default();
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
        .min_size([620.0, 420.0])
        .default_size([760.0, 560.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            egui::Panel::bottom("settings-actions")
                .exact_size(36.0)
                .show(ui, |ui| {
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
            egui::Panel::left("settings-categories")
                .exact_size(170.0)
                .resizable(false)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("SETTINGS").small().strong().weak());
                    ui.add_space(10.0);
                    for cat in Category::ALL {
                        if ui
                            .add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::selectable(
                                    state.dialogs.settings.category == cat,
                                    cat.label(),
                                ),
                            )
                            .clicked()
                        {
                            state.dialogs.settings.category = cat;
                        }
                    }
                });
            egui::CentralPanel::no_frame().show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("settings-content")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                match state.dialogs.settings.category {
                                    Category::Appearance => appearance_tab(ui, state),
                                    Category::View => view_tab(ui, state),
                                    Category::Http => {
                                        if let Some(draft) = state.dialogs.settings.draft.as_mut() {
                                            http_tab(ui, draft);
                                        }
                                    }
                                    Category::Editor => editor_tab(ui, state),
                                    #[cfg(feature = "pro")]
                                    Category::Jira => jira_tab(ui, state),
                                    Category::Keymap => keymap_tab(ui),
                                }
                            });
                    });
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
    ui.add_space(12.0);
    settings_card(ui, "Interface", |ui| {
        egui::Grid::new("appearance-grid")
            .num_columns(2)
            .spacing([18.0, 12.0])
            .show(ui, |ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("settings-theme")
                    .selected_text(state.theme.label())
                    .show_ui(ui, |ui| {
                        for kind in ThemeKind::ALL {
                            if ui
                                .selectable_value(&mut state.theme, kind, kind.label())
                                .clicked()
                            {
                                kind.apply(ui.ctx());
                                apply_typography(ui.ctx(), state);
                            }
                        }
                    });
                ui.end_row();

                ui.label("UI font");
                egui::ComboBox::from_id_salt("settings-ui-font")
                    .selected_text(state.ui_font.label())
                    .show_ui(ui, |ui| {
                        for font in UiFont::ALL {
                            ui.selectable_value(&mut state.ui_font, font, font.label());
                        }
                    });
                ui.end_row();

                ui.label("UI font size");
                ui.add(egui::Slider::new(&mut state.ui_font_size, 11.0..=20.0).suffix(" px"));
                ui.end_row();
            });
    });
    apply_typography(ui.ctx(), state);
}

fn editor_tab(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Editor");
    ui.add_space(12.0);
    settings_card(ui, "Editing", |ui| {
        ui.horizontal(|ui| {
            ui.label("Code font size");
            ui.add(egui::Slider::new(&mut state.editor_font_size, 9.0..=24.0).suffix(" px"));
        });
        ui.checkbox(
            &mut state.auto_save,
            "Save dirty files when switching or closing",
        );
    });
    apply_typography(ui.ctx(), state);
}

pub fn apply_typography(ctx: &egui::Context, state: &AppState) {
    let family = match state.ui_font {
        UiFont::Sans => egui::FontFamily::Proportional,
        UiFont::Monospace => egui::FontFamily::Monospace,
    };
    ctx.all_styles_mut(|style| {
        for text_style in [
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Small,
        ] {
            if let Some(id) = style.text_styles.get_mut(&text_style) {
                id.family = family.clone();
                id.size = if text_style == egui::TextStyle::Small {
                    (state.ui_font_size - 1.0).max(10.0)
                } else {
                    state.ui_font_size
                };
            }
        }
        if let Some(id) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
            id.family = family.clone();
            id.size = state.ui_font_size + 8.0;
        }
        style.text_styles.insert(
            crate::widgets::code_editor::editor_text_style(),
            egui::FontId::new(state.editor_font_size, egui::FontFamily::Monospace),
        );
    });
}

fn view_tab(ui: &mut Ui, state: &mut AppState) {
    ui.heading("View");
    ui.add_space(12.0);
    settings_card(ui, "Tool windows", |ui| {
        ui.checkbox(&mut state.show_activity_bar, "Activity bar");
        ui.checkbox(&mut state.show_assets, "Project");
        ui.checkbox(&mut state.show_collections, "Collections");
        ui.checkbox(&mut state.show_environment, "Environment");
        ui.checkbox(&mut state.show_bottom_bar, "Bottom tool bar");
        ui.checkbox(&mut state.show_status_bar, "Status bar");
    });
    ui.add_space(10.0);
    settings_card(ui, "Focus", |ui| {
        ui.checkbox(&mut state.zen_mode, "Zen mode");
        ui.label("Ctrl+Shift+F11")
            .on_hover_text("Zen mode hides chrome; move to an edge to reveal its tool window.");
    });
}

#[cfg(feature = "pro")]
fn jira_tab(ui: &mut Ui, state: &mut AppState) {
    ui.heading("Jira");
    ui.add_space(12.0);
    state.dialogs.jira.ensure_config();
    let mut save = false;
    settings_card(ui, "Connection", |ui| {
        ui.label("Used by the Jira integration (ticket details, comments) — a ApiWright Pro feature.");
        ui.add_space(8.0);
        egui::Grid::new("jira-settings-grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Base URL");
                ui.add(
                    egui::TextEdit::singleline(&mut state.dialogs.jira.config.base_url)
                        .desired_width(320.0)
                        .hint_text("https://yourcompany.atlassian.net"),
                );
                ui.end_row();
                ui.label("Email").on_hover_text(
                    "Jira Cloud: account email for the API token. Leave empty on Server/Data Center to use a personal access token.",
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.dialogs.jira.config.email)
                        .desired_width(320.0)
                        .hint_text("you@company.com (Cloud) — empty for Server/DC"),
                );
                ui.end_row();
                ui.label("API token");
                ui.add(
                    egui::TextEdit::singleline(&mut state.dialogs.jira.config.api_token)
                        .desired_width(320.0)
                        .password(true),
                );
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.weak("Stored in ~/.config/forge/jira.json (owner-readable only), never in the project.");
        ui.add_space(4.0);
        if ui.button("Save connection").clicked() {
            save = true;
        }
    });
    if save {
        match crate::jira::save_config(&state.dialogs.jira.config) {
            Ok(()) => state.status = Some(StatusMessage::info("Jira connection saved")),
            Err(error) => state.status = Some(StatusMessage::error(error)),
        }
    }
}

fn settings_card(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.strong(title);
            ui.add_space(10.0);
            add_contents(ui);
        });
}

fn http_tab(ui: &mut Ui, draft: &mut HttpDraft) {
    ui.heading("HTTP");
    ui.add_space(12.0);
    settings_card(ui, "Network", |ui| {
        egui::Grid::new("http-settings-grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Timeout");
                ui.add(
                    egui::DragValue::new(&mut draft.timeout_ms)
                        .range(1..=600_000)
                        .suffix(" ms"),
                );
                ui.end_row();
                ui.label("Redirects");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut draft.follow_redirects, "Follow");
                    ui.add(
                        egui::DragValue::new(&mut draft.max_redirects)
                            .range(0..=50)
                            .suffix(" max"),
                    );
                });
                ui.end_row();
                ui.label("TLS");
                ui.checkbox(&mut draft.verify_tls, "Verify certificates");
                ui.end_row();
            });
        text_row(ui, "User-Agent", &mut draft.user_agent, "default");
    });
    ui.add_space(10.0);
    settings_card(ui, "Client certificate", |ui| {
        ui.label("Workspace-relative or absolute PEM paths")
            .on_hover_text("The private key may be embedded in the client certificate file.");
        path_row(
            ui,
            "Certificate",
            &mut draft.tls_client_cert,
            "certs/client.pem",
        );
        path_row(ui, "Private key", &mut draft.tls_client_key, "optional");
        path_row(
            ui,
            "CA bundle",
            &mut draft.tls_ca_bundle,
            "certs/internal-ca.pem",
        );
    });
    ui.add_space(10.0);
    settings_card(ui, "OpenAPI", |ui| {
        ui.add(
            TextEdit::singleline(&mut draft.openapi_url)
                .hint_text("https://api.example.com/openapi.json or specs/api.yaml")
                .desired_width(ui.available_width()),
        )
        .on_hover_text("Project fallback; folder properties can override this source.");
    });
    ui.add_space(10.0);
    settings_card(ui, "Proxy", |ui| {
        ui.checkbox(&mut draft.proxy_enabled, "Use proxy");
        ui.add_enabled_ui(draft.proxy_enabled, |ui| {
            text_row(ui, "URL", &mut draft.proxy_url, "http://127.0.0.1:8080");
            text_row(
                ui,
                "No proxy",
                &mut draft.proxy_no_proxy,
                "comma-separated host suffixes",
            );
        });
    });
}

fn text_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new(label),
        );
        ui.add(
            TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(ui.available_width()),
        );
    });
}

fn path_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [100.0, ui.spacing().interact_size.y],
            egui::Label::new(label),
        );
        let field_width = (ui.available_width() - 80.0).max(100.0);
        ui.add(
            TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(field_width),
        );
        if ui.button("Browse…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("PEM", &["pem", "crt", "key"])
                .pick_file()
            {
                *value = path.display().to_string();
            }
        }
    });
}

fn keymap_tab(ui: &mut Ui) {
    ui.heading("Keymap");
    ui.add_space(8.0);
    egui::Grid::new("keymap-grid")
        .num_columns(2)
        .striped(true)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_font_size_does_not_change_generic_monospace_text() {
        let ctx = egui::Context::default();
        let before = ctx.style_of(egui::Theme::Dark).text_styles[&egui::TextStyle::Monospace].size;
        let mut state = AppState::default();
        state.editor_font_size = 22.0;

        apply_typography(&ctx, &state);

        assert_eq!(
            ctx.style_of(egui::Theme::Dark).text_styles[&egui::TextStyle::Monospace].size,
            before
        );
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).text_styles
                [&crate::widgets::code_editor::editor_text_style()]
                .size,
            22.0
        );
    }
}
