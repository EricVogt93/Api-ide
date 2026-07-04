//! The request editor: method/URL bar, the flat sub-tab strip (Params,
//! Headers, Auth, Body, Assertions, Extract, Scripts, Settings) and the
//! vertical splitter down to the response viewer.

use std::collections::BTreeMap;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, TextBuffer, TextEdit, Ui};

use forge_core::assert::{generate_from_response, GenerateOptions};
use forge_core::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, ExtractScope, Extractor, ExtractorSource, KeyValue, Method,
    MultipartPart, Param, ParamKind, PartContent, RawLanguage, RequestDef,
};
use forge_core::runner::{RunOptions, RunScope};
use forge_core::store::{TreeNode, Workspace};
use forge_core::vars::{spans, VarScopes};

use crate::bridge::{Bridge, Cmd};
use crate::state::{AppState, RequestSubTab, RunState, StatusMessage, Tab};
use crate::theme::ThemeKind;
use crate::widgets::code_editor::{code_editor, Lang};
use crate::widgets::kv_table::kv_table;
use crate::widgets::method_badge::method_color;
use crate::widgets::response_view::response_view;
use crate::widgets::underline_tabs;

/// Render the request editor + response viewer for the active tab.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    let mut send_clicked = false;
    let mut stop_clicked = false;

    let mut export_def: Option<RequestDef> = None;

    {
        let AppState { workspace, tabs, active_tab, active_env, theme, .. } = state;
        let Some(idx) = *active_tab else {
            ui.centered_and_justified(|ui| ui.weak("Open a request to get started."));
            return;
        };
        let Some(tab) = tabs.get_mut(idx) else { return };
        let scopes = workspace
            .as_ref()
            .map(|ws| build_scopes(ws, &tab.rel_id, active_env.as_deref()))
            .unwrap_or_default();
        let theme = *theme;

        // Scope every editor widget id (url bar, method combo, code editors,
        // kv tables, ...) below to this tab's `rel_id`, so egui's per-widget
        // state (in particular `TextEdit` undo history) can't leak across
        // tabs: without this, switching tabs and immediately pressing
        // Ctrl+Z could paste the previously active tab's text into this one.
        ui.push_id(egui::Id::new(&tab.rel_id), |ui| {
            render_tab(ui, tab, &scopes, theme, &mut send_clicked, &mut stop_clicked, &mut export_def);
        });
    }

    if let Some(def) = export_def {
        state.dialogs.snippet_export.open(def);
    }
    if send_clicked {
        send_active(state, bridge);
    }
    if stop_clicked {
        if let Some(run_id) = state.active_tab_ref().and_then(|t| t.run_id) {
            bridge.send(Cmd::Cancel { run_id });
        }
    }
}

/// Render one tab's method/URL bar, sub-tab strip, splitter and response
/// viewer. Callers are expected to have already scoped widget ids to the
/// tab (see the `ui.push_id` in [`show`]).
#[allow(clippy::too_many_arguments)]
fn render_tab(
    ui: &mut Ui,
    tab: &mut Tab,
    scopes: &VarScopes,
    theme: ThemeKind,
    send_clicked: &mut bool,
    stop_clicked: &mut bool,
    export_def: &mut Option<RequestDef>,
) {
    let total_height = ui.available_height();
    let top_height = (total_height * tab.split_ratio).clamp(120.0, (total_height - 80.0).max(120.0));

    ui.allocate_ui(egui::vec2(ui.available_width(), top_height), |ui| {
        egui::ScrollArea::vertical().id_salt("request-editor-scroll").show(ui, |ui| {
            let dark = theme.editor_bg().r() < 128;

            ui.horizontal(|ui| {
                let mut method = tab.def.method;
                egui::ComboBox::from_id_salt("method-combo")
                    .selected_text(RichText::new(method.as_str()).color(method_color(method)).strong())
                    .width(85.0)
                    .show_ui(ui, |ui| {
                        for m in Method::ALL {
                            ui.selectable_value(&mut method, m, m.as_str());
                        }
                    });
                if method != tab.def.method {
                    tab.def.method = method;
                    tab.dirty = true;
                }

                let button_w = 64.0;
                let url_width = (ui.available_width() - button_w - 8.0).max(100.0);
                let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                    let mut job = url_layout_job(buf.as_str(), scopes, dark);
                    job.wrap.max_width = wrap_width;
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let resp = ui.add(
                    TextEdit::singleline(&mut tab.def.url)
                        .id_salt("url-bar")
                        .desired_width(url_width)
                        .font(egui::FontSelection::from(FontId::monospace(13.0)))
                        .layouter(&mut layouter),
                );
                if resp.changed() {
                    tab.dirty = true;
                }

                if tab.run_id.is_some() {
                    ui.spinner();
                    let stop = egui::Button::new(
                        egui::RichText::new(format!("{} Stop", crate::theme::icons::STOP)).color(egui::Color32::WHITE),
                    )
                    .fill(theme.error_color());
                    if ui.add(stop).clicked() {
                        *stop_clicked = true;
                    }
                } else {
                    // New-UI-style primary action: accent-filled Send button.
                    let send = egui::Button::new(
                        egui::RichText::new(format!("{} Send", crate::theme::icons::PLAY))
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(theme.accent_color());
                    if ui.add(send).clicked() {
                        *send_clicked = true;
                    }
                }
                if ui.button("Export code...").clicked() {
                    *export_def = Some(tab.def.clone());
                }
            });

            ui.add_space(4.0);
            let tabs_list: &[(RequestSubTab, &str)] = &[
                (RequestSubTab::Params, "Params"),
                (RequestSubTab::Headers, "Headers"),
                (RequestSubTab::Auth, "Auth"),
                (RequestSubTab::Body, "Body"),
                (RequestSubTab::Assertions, "Assertions"),
                (RequestSubTab::Extract, "Extract"),
                (RequestSubTab::Scripts, "Scripts"),
                (RequestSubTab::Settings, "Settings"),
            ];
            underline_tabs(ui, tabs_list, &mut tab.sub_tab);
            ui.add_space(4.0);

            let changed = match tab.sub_tab {
                RequestSubTab::Params => params_tab(ui, &mut tab.def.params),
                RequestSubTab::Headers => kv_table(ui, "req-headers", &mut tab.def.headers, true),
                RequestSubTab::Auth => auth_tab(ui, &mut tab.def.auth),
                RequestSubTab::Body => body_tab(ui, &mut tab.def.body, scopes),
                RequestSubTab::Assertions => assertions_tab(ui, &mut tab.def, tab.response.as_ref()),
                RequestSubTab::Extract => extract_tab(ui, &mut tab.def.extractors),
                RequestSubTab::Scripts => scripts_tab(ui, &mut tab.def, scopes),
                RequestSubTab::Settings => settings_tab(ui, &mut tab.def.settings),
            };
            if changed {
                tab.dirty = true;
            }
        });
    });

    let splitter = ui.allocate_response(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
    ui.painter().hline(
        splitter.rect.x_range(),
        splitter.rect.center().y,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    if splitter.dragged() && total_height > 1.0 {
        tab.split_ratio = ((top_height + splitter.drag_delta().y) / total_height).clamp(0.15, 0.85);
    }
    if splitter.hovered() || splitter.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    ui.allocate_ui(egui::vec2(ui.available_width(), ui.available_height()), |ui| {
        response_view(ui, tab.response.as_ref(), &mut tab.response_state, theme);
    });
}

pub fn send_active(state: &mut AppState, bridge: &Bridge) {
    let Some(idx) = state.active_tab else { return };
    let Some(workspace) = state.workspace.clone() else {
        state.status = Some(StatusMessage::error("No workspace open"));
        return;
    };
    let rel_id = state.tabs[idx].rel_id.clone();
    let scope = RunScope::Request(rel_id);
    let options = RunOptions { environment: state.active_env.clone(), ..Default::default() };
    state.last_run = Some((scope.clone(), options.clone()));
    let run_id = state.alloc_run_id();
    state.tabs[idx].run_id = Some(run_id);
    state.run_state = RunState { run_id: Some(run_id), total: 1, completed: 0 };
    state.run_log.start(run_id);
    bridge.send(Cmd::Run { run_id, workspace: Box::new(workspace), scope, options });
}

/// Build a best-effort variable scope for editor highlighting: the active
/// environment plus the collection/folder chain that owns `rel_id`. This is
/// a preview aid only — the authoritative resolution happens in
/// `forge_core::runner` when the request actually executes.
fn build_scopes(workspace: &Workspace, rel_id: &str, active_env: Option<&str>) -> VarScopes {
    let mut scopes = VarScopes::new();
    if let Some(env_name) = active_env {
        if let Some(loaded) = workspace.environment(env_name) {
            scopes = scopes.with_environment(&loaded.env, &loaded.secrets);
        }
    }
    for col in &workspace.collections {
        if let Some(folder_vars) = find_ancestor_vars(&col.children, rel_id, workspace) {
            scopes = scopes.with_collection(&col.meta.variables).with_folders(folder_vars.iter().copied());
            return scopes;
        }
    }
    scopes
}

fn find_ancestor_vars<'a>(
    children: &'a [TreeNode],
    rel_id: &str,
    workspace: &Workspace,
) -> Option<Vec<&'a BTreeMap<String, String>>> {
    for child in children {
        match child {
            TreeNode::Request(r) if workspace.rel_id(&r.file) == rel_id => return Some(Vec::new()),
            TreeNode::Request(_) => {}
            TreeNode::Folder(f) => {
                if let Some(mut acc) = find_ancestor_vars(&f.children, rel_id, workspace) {
                    acc.push(&f.meta.variables);
                    return Some(acc);
                }
            }
        }
    }
    None
}

fn url_layout_job(text: &str, scopes: &VarScopes, dark: bool) -> LayoutJob {
    let (base, var_fg) = if dark {
        (Color32::from_rgb(0xA9, 0xB7, 0xC6), Color32::from_rgb(0xFF, 0xC6, 0x6D))
    } else {
        (Color32::BLACK, Color32::from_rgb(0xB3, 0x6B, 0x00))
    };
    let font = FontId::monospace(13.0);
    let var_spans = spans(text, scopes);
    let mut job = LayoutJob::default();
    let mut cursor = 0usize;
    for v in &var_spans {
        if v.start > cursor {
            job.append(&text[cursor..v.start], 0.0, TextFormat::simple(font.clone(), base));
        }
        let mut fmt = TextFormat::simple(font.clone(), var_fg);
        fmt.background = Color32::from_rgba_unmultiplied(var_fg.r(), var_fg.g(), var_fg.b(), 30);
        job.append(&text[v.start..v.end], 0.0, fmt);
        cursor = v.end;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, TextFormat::simple(font, base));
    }
    job
}

fn params_tab(ui: &mut Ui, params: &mut Vec<Param>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;

    if params.last().is_none_or(|p| !p.kv.key.is_empty() || !p.kv.value.is_empty()) {
        params.push(Param { kv: KeyValue::new("", ""), kind: ParamKind::Query });
    }

    egui::Grid::new("params-grid").num_columns(6).striped(true).show(ui, |ui| {
        ui.strong("");
        ui.strong("Key");
        ui.strong("Value");
        ui.strong("Type");
        ui.strong("Description");
        ui.strong("");
        ui.end_row();

        let n = params.len();
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            if ui.checkbox(&mut params[i].kv.enabled, "").changed() {
                changed = true;
            }
            if ui.text_edit_singleline(&mut params[i].kv.key).changed() {
                changed = true;
            }
            if ui.text_edit_singleline(&mut params[i].kv.value).changed() {
                changed = true;
            }
            let mut kind = params[i].kind;
            egui::ComboBox::from_id_salt(("param-kind", i))
                .selected_text(if kind == ParamKind::Query { "Query" } else { "Path" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut kind, ParamKind::Query, "Query");
                    ui.selectable_value(&mut kind, ParamKind::Path, "Path");
                });
            if kind != params[i].kind {
                params[i].kind = kind;
                changed = true;
            }
            if ui.text_edit_singleline(&mut params[i].kv.description).changed() {
                changed = true;
            }
            let is_trailing_blank = i == n - 1 && params[i].kv.key.is_empty() && params[i].kv.value.is_empty();
            if !is_trailing_blank && ui.small_button("\u{2715}").clicked() {
                remove = Some(i);
            }
            ui.end_row();
        }
    });

    if let Some(i) = remove {
        params.remove(i);
        changed = true;
    }
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    None,
    Inherit,
    Basic,
    Bearer,
    ApiKey,
    OAuth2ClientCredentials,
    OAuth2AuthCode,
}

impl AuthKind {
    const ALL: [AuthKind; 7] = [
        AuthKind::Inherit,
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::ApiKey,
        AuthKind::OAuth2ClientCredentials,
        AuthKind::OAuth2AuthCode,
    ];

    fn label(&self) -> &'static str {
        match self {
            AuthKind::None => "None",
            AuthKind::Inherit => "Inherit",
            AuthKind::Basic => "Basic",
            AuthKind::Bearer => "Bearer Token",
            AuthKind::ApiKey => "API Key",
            AuthKind::OAuth2ClientCredentials => "OAuth 2.0 (Client Credentials)",
            AuthKind::OAuth2AuthCode => "OAuth 2.0 (Authorization Code)",
        }
    }

    fn of(auth: &AuthConfig) -> Self {
        match auth {
            AuthConfig::None => AuthKind::None,
            AuthConfig::Inherit => AuthKind::Inherit,
            AuthConfig::Basic { .. } => AuthKind::Basic,
            AuthConfig::Bearer { .. } => AuthKind::Bearer,
            AuthConfig::ApiKey { .. } => AuthKind::ApiKey,
            AuthConfig::OAuth2ClientCredentials { .. } => AuthKind::OAuth2ClientCredentials,
            AuthConfig::OAuth2AuthCode { .. } => AuthKind::OAuth2AuthCode,
        }
    }

    fn default_config(&self) -> AuthConfig {
        match self {
            AuthKind::None => AuthConfig::None,
            AuthKind::Inherit => AuthConfig::Inherit,
            AuthKind::Basic => AuthConfig::Basic { username: String::new(), password: String::new() },
            AuthKind::Bearer => AuthConfig::Bearer { token: String::new(), prefix: None },
            AuthKind::ApiKey => AuthConfig::ApiKey {
                key: String::new(),
                value: String::new(),
                placement: ApiKeyPlacement::Header,
            },
            AuthKind::OAuth2ClientCredentials => AuthConfig::OAuth2ClientCredentials {
                token_url: String::new(),
                client_id: String::new(),
                client_secret: String::new(),
                scopes: Vec::new(),
                credentials_in_body: false,
            },
            AuthKind::OAuth2AuthCode => AuthConfig::OAuth2AuthCode {
                auth_url: String::new(),
                token_url: String::new(),
                client_id: String::new(),
                client_secret: None,
                scopes: Vec::new(),
                redirect_port: None,
                pkce: true,
            },
        }
    }
}

fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([120.0, ui.spacing().interact_size.y], egui::Label::new(label));
        add(ui);
    });
}

fn auth_tab(ui: &mut Ui, auth: &mut AuthConfig) -> bool {
    let mut changed = false;
    let mut kind = AuthKind::of(auth);
    let prev = kind;
    egui::ComboBox::from_id_salt("auth-kind").selected_text(kind.label()).show_ui(ui, |ui| {
        for k in AuthKind::ALL {
            ui.selectable_value(&mut kind, k, k.label());
        }
    });
    if kind != prev {
        *auth = kind.default_config();
        changed = true;
    }
    ui.add_space(6.0);

    match auth {
        AuthConfig::None => {
            ui.weak("No credentials sent by this request.");
        }
        AuthConfig::Inherit => {
            ui.weak("Uses the nearest folder/collection auth (or none).");
        }
        AuthConfig::Basic { username, password } => {
            field(ui, "Username", |ui| changed |= ui.text_edit_singleline(username).changed());
            field(ui, "Password", |ui| {
                changed |= ui.add(TextEdit::singleline(password).password(true)).changed()
            });
        }
        AuthConfig::Bearer { token, prefix } => {
            field(ui, "Token", |ui| changed |= ui.add(TextEdit::singleline(token).password(true)).changed());
            let mut p = prefix.clone().unwrap_or_else(|| "Bearer".to_string());
            field(ui, "Prefix", |ui| {
                if ui.text_edit_singleline(&mut p).changed() {
                    *prefix = if p.is_empty() || p == "Bearer" { None } else { Some(p.clone()) };
                    changed = true;
                }
            });
        }
        AuthConfig::ApiKey { key, value, placement } => {
            field(ui, "Key", |ui| changed |= ui.text_edit_singleline(key).changed());
            field(ui, "Value", |ui| changed |= ui.add(TextEdit::singleline(value).password(true)).changed());
            field(ui, "Add to", |ui| {
                egui::ComboBox::from_id_salt("apikey-placement")
                    .selected_text(if *placement == ApiKeyPlacement::Header { "Header" } else { "Query" })
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(placement, ApiKeyPlacement::Header, "Header").changed() {
                            changed = true;
                        }
                        if ui.selectable_value(placement, ApiKeyPlacement::Query, "Query").changed() {
                            changed = true;
                        }
                    });
            });
        }
        AuthConfig::OAuth2ClientCredentials { token_url, client_id, client_secret, credentials_in_body, .. } => {
            field(ui, "Token URL", |ui| changed |= ui.text_edit_singleline(token_url).changed());
            field(ui, "Client ID", |ui| changed |= ui.text_edit_singleline(client_id).changed());
            field(ui, "Client Secret", |ui| {
                changed |= ui.add(TextEdit::singleline(client_secret).password(true)).changed()
            });
            field(ui, "Credentials in body", |ui| changed |= ui.checkbox(credentials_in_body, "").changed());
        }
        AuthConfig::OAuth2AuthCode { auth_url, token_url, client_id, client_secret, pkce, .. } => {
            field(ui, "Auth URL", |ui| changed |= ui.text_edit_singleline(auth_url).changed());
            field(ui, "Token URL", |ui| changed |= ui.text_edit_singleline(token_url).changed());
            field(ui, "Client ID", |ui| changed |= ui.text_edit_singleline(client_id).changed());
            let mut secret = client_secret.clone().unwrap_or_default();
            field(ui, "Client Secret", |ui| {
                if ui.add(TextEdit::singleline(&mut secret).password(true)).changed() {
                    *client_secret = if secret.is_empty() { None } else { Some(secret.clone()) };
                    changed = true;
                }
            });
            field(ui, "PKCE", |ui| changed |= ui.checkbox(pkce, "").changed());
        }
    }
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    None,
    Json,
    Xml,
    Raw,
    Form,
    Multipart,
    GraphQl,
    Binary,
}

impl BodyKind {
    const ALL: [BodyKind; 8] = [
        BodyKind::None,
        BodyKind::Json,
        BodyKind::Xml,
        BodyKind::Raw,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::GraphQl,
        BodyKind::Binary,
    ];

    fn label(&self) -> &'static str {
        match self {
            BodyKind::None => "None",
            BodyKind::Json => "JSON",
            BodyKind::Xml => "XML",
            BodyKind::Raw => "Raw",
            BodyKind::Form => "Form URL-encoded",
            BodyKind::Multipart => "Multipart",
            BodyKind::GraphQl => "GraphQL",
            BodyKind::Binary => "Binary",
        }
    }

    fn of(body: &BodyDef) -> Self {
        match body {
            BodyDef::None => BodyKind::None,
            BodyDef::Json { .. } => BodyKind::Json,
            BodyDef::Xml { .. } => BodyKind::Xml,
            BodyDef::Raw { .. } => BodyKind::Raw,
            BodyDef::FormUrlencoded { .. } => BodyKind::Form,
            BodyDef::Multipart { .. } => BodyKind::Multipart,
            BodyDef::GraphQl { .. } => BodyKind::GraphQl,
            BodyDef::Binary { .. } => BodyKind::Binary,
        }
    }

    fn default_body(&self) -> BodyDef {
        match self {
            BodyKind::None => BodyDef::None,
            BodyKind::Json => BodyDef::Json { text: String::new() },
            BodyKind::Xml => BodyDef::Xml { text: String::new() },
            BodyKind::Raw => BodyDef::Raw { text: String::new(), language: RawLanguage::Text },
            BodyKind::Form => BodyDef::FormUrlencoded { fields: Vec::new() },
            BodyKind::Multipart => BodyDef::Multipart { parts: Vec::new() },
            BodyKind::GraphQl => BodyDef::GraphQl { query: String::new(), variables: String::new(), operation_name: None },
            BodyKind::Binary => BodyDef::Binary { path: String::new() },
        }
    }
}

fn body_tab(ui: &mut Ui, body: &mut BodyDef, scopes: &VarScopes) -> bool {
    let mut changed = false;
    let mut kind = BodyKind::of(body);
    let prev = kind;
    egui::ComboBox::from_id_salt("body-kind").selected_text(kind.label()).show_ui(ui, |ui| {
        for k in BodyKind::ALL {
            ui.selectable_value(&mut kind, k, k.label());
        }
    });
    if kind != prev {
        *body = kind.default_body();
        changed = true;
    }
    ui.add_space(4.0);

    match body {
        BodyDef::None => {
            ui.weak("This request has no body.");
        }
        BodyDef::Json { text } => {
            if code_editor(ui, "body-json", text, Lang::Json, Some(scopes), false, 10, true).changed() {
                changed = true;
            }
        }
        BodyDef::Xml { text } => {
            if code_editor(ui, "body-xml", text, Lang::Xml, Some(scopes), false, 10, true).changed() {
                changed = true;
            }
        }
        BodyDef::Raw { text, language } => {
            let mut lang = *language;
            egui::ComboBox::from_id_salt("raw-lang")
                .selected_text(format!("{lang:?}"))
                .show_ui(ui, |ui| {
                    for l in [RawLanguage::Text, RawLanguage::Json, RawLanguage::Xml, RawLanguage::Html, RawLanguage::Yaml] {
                        ui.selectable_value(&mut lang, l, format!("{l:?}"));
                    }
                });
            if lang != *language {
                *language = lang;
                changed = true;
            }
            let editor_lang = match language {
                RawLanguage::Json => Lang::Json,
                RawLanguage::Xml | RawLanguage::Html => Lang::Xml,
                RawLanguage::Text | RawLanguage::Yaml => Lang::Plain,
            };
            if code_editor(ui, "body-raw", text, editor_lang, Some(scopes), false, 10, true).changed() {
                changed = true;
            }
        }
        BodyDef::FormUrlencoded { fields } => {
            if kv_table(ui, "body-form", fields, true) {
                changed = true;
            }
        }
        BodyDef::Multipart { parts } => {
            if multipart_editor(ui, parts) {
                changed = true;
            }
        }
        BodyDef::GraphQl { query, variables, operation_name } => {
            ui.label("Query");
            if code_editor(ui, "gql-query", query, Lang::GraphQl, Some(scopes), false, 8, true).changed() {
                changed = true;
            }
            ui.label("Variables (JSON)");
            if code_editor(ui, "gql-vars", variables, Lang::Json, Some(scopes), false, 4, true).changed() {
                changed = true;
            }
            let mut op = operation_name.clone().unwrap_or_default();
            field(ui, "Operation name", |ui| {
                if ui.text_edit_singleline(&mut op).changed() {
                    *operation_name = if op.is_empty() { None } else { Some(op.clone()) };
                    changed = true;
                }
            });
        }
        BodyDef::Binary { path } => {
            ui.horizontal(|ui| {
                if ui.text_edit_singleline(path).changed() {
                    changed = true;
                }
                if ui.button("Browse...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                        *path = p.display().to_string();
                        changed = true;
                    }
                }
            });
        }
    }
    changed
}

fn multipart_editor(ui: &mut Ui, parts: &mut Vec<MultipartPart>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    for (i, part) in parts.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut part.enabled, "").changed() {
                changed = true;
            }
            if ui.text_edit_singleline(&mut part.name).changed() {
                changed = true;
            }
            let is_file = matches!(part.content, PartContent::File { .. });
            if ui.selectable_label(!is_file, "Text").clicked() && is_file {
                part.content = PartContent::Text { value: String::new() };
                changed = true;
            }
            if ui.selectable_label(is_file, "File").clicked() && !is_file {
                part.content = PartContent::File { path: String::new() };
                changed = true;
            }
            match &mut part.content {
                PartContent::Text { value } => {
                    if ui.text_edit_singleline(value).changed() {
                        changed = true;
                    }
                }
                PartContent::File { path } => {
                    if ui.text_edit_singleline(path).changed() {
                        changed = true;
                    }
                    if ui.button("Browse...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            *path = p.display().to_string();
                            changed = true;
                        }
                    }
                }
            }
            let mut ct = part.content_type.clone().unwrap_or_default();
            if ui.text_edit_singleline(&mut ct).changed() {
                part.content_type = if ct.is_empty() { None } else { Some(ct) };
                changed = true;
            }
            if ui.small_button("\u{2715}").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        parts.remove(i);
        changed = true;
    }
    if ui.button("+ Add part").clicked() {
        parts.push(MultipartPart { name: String::new(), content: PartContent::Text { value: String::new() }, content_type: None, enabled: true });
        changed = true;
    }
    changed
}

fn assertions_tab(ui: &mut Ui, def: &mut RequestDef, response: Option<&forge_core::runner::RequestOutcome>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;

    egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
        if def.assertions.is_empty() {
            ui.weak("No assertions yet.");
        }
        for (i, a) in def.assertions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut a.enabled, "").changed() {
                    changed = true;
                }
                ui.label(a.check.summary());
                if ui.small_button("\u{2715}").clicked() {
                    remove = Some(i);
                }
            });
        }
    });
    if let Some(i) = remove {
        def.assertions.remove(i);
        changed = true;
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.menu_button("+ Add", |ui| {
            for (label, check) in default_checks() {
                if ui.button(label).clicked() {
                    def.assertions.push(check.into());
                    changed = true;
                    ui.close();
                }
            }
        });

        let can_generate = matches!(response.map(|r| &r.result), Some(Ok(_)));
        if ui.add_enabled(can_generate, egui::Button::new("Generate from response")).clicked() {
            if let Some(Ok(exec)) = response.map(|r| &r.result) {
                let checks = generate_from_response(exec, &GenerateOptions::default());
                def.assertions.extend(checks.into_iter().map(Into::into));
                changed = true;
            }
        }
    });
    changed
}

fn default_checks() -> Vec<(&'static str, forge_core::model::Check)> {
    use forge_core::model::{Check, NumberOp, ValueOp};
    vec![
        ("Status code", Check::StatusCode { op: NumberOp::Eq, value: 200 }),
        ("Status class (2xx)", Check::StatusClass { class: 2 }),
        ("Header", Check::Header { name: String::new(), op: forge_core::model::StringOp::Exists, value: String::new() }),
        ("Content-Type", Check::ContentType { value: "application/json".to_string() }),
        ("JSON path", Check::JsonPath { path: "$.".to_string(), op: ValueOp::Exists, value: serde_json::Value::Null }),
        ("Body contains", Check::BodyContains { value: String::new() }),
        ("Body matches regex", Check::BodyMatches { regex: String::new() }),
        ("Response time below", Check::ResponseTimeBelow { max_ms: 1000 }),
        ("JSON schema", Check::JsonSchema { schema: serde_json::json!({}) }),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    JsonPath,
    Header,
    Regex,
}

impl SourceKind {
    const ALL: [SourceKind; 3] = [SourceKind::JsonPath, SourceKind::Header, SourceKind::Regex];

    fn label(&self) -> &'static str {
        match self {
            SourceKind::JsonPath => "JSON Path",
            SourceKind::Header => "Header",
            SourceKind::Regex => "Regex",
        }
    }

    fn of(source: &ExtractorSource) -> Self {
        match source {
            ExtractorSource::JsonPath { .. } => SourceKind::JsonPath,
            ExtractorSource::Header { .. } => SourceKind::Header,
            ExtractorSource::Regex { .. } => SourceKind::Regex,
        }
    }

    fn default_source(&self) -> ExtractorSource {
        match self {
            SourceKind::JsonPath => ExtractorSource::JsonPath { expr: "$.".to_string() },
            SourceKind::Header => ExtractorSource::Header { name: String::new() },
            SourceKind::Regex => ExtractorSource::Regex { pattern: String::new(), group: 0 },
        }
    }
}

fn extract_tab(ui: &mut Ui, extractors: &mut Vec<Extractor>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;

    for (i, ext) in extractors.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut ext.enabled, "").changed() {
                changed = true;
            }
            let mut kind = SourceKind::of(&ext.source);
            let prev = kind;
            egui::ComboBox::from_id_salt(("extract-kind", i)).selected_text(kind.label()).show_ui(ui, |ui| {
                for k in SourceKind::ALL {
                    ui.selectable_value(&mut kind, k, k.label());
                }
            });
            if kind != prev {
                ext.source = kind.default_source();
                changed = true;
            }
            match &mut ext.source {
                ExtractorSource::JsonPath { expr } => {
                    if ui.text_edit_singleline(expr).changed() {
                        changed = true;
                    }
                }
                ExtractorSource::Header { name } => {
                    if ui.text_edit_singleline(name).changed() {
                        changed = true;
                    }
                }
                ExtractorSource::Regex { pattern, group } => {
                    if ui.text_edit_singleline(pattern).changed() {
                        changed = true;
                    }
                    ui.label("group");
                    let mut g = *group as i64;
                    if ui.add(egui::DragValue::new(&mut g).range(0..=20)).changed() {
                        *group = g.max(0) as usize;
                        changed = true;
                    }
                }
            }
            ui.label("\u{2192} var:");
            if ui.text_edit_singleline(&mut ext.var).changed() {
                changed = true;
            }
            let mut scope = ext.scope;
            egui::ComboBox::from_id_salt(("extract-scope", i))
                .selected_text(if scope == ExtractScope::Runtime { "Runtime" } else { "Environment" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut scope, ExtractScope::Runtime, "Runtime");
                    ui.selectable_value(&mut scope, ExtractScope::Environment, "Environment");
                });
            if scope != ext.scope {
                ext.scope = scope;
                changed = true;
            }
            if ui.small_button("\u{2715}").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        extractors.remove(i);
        changed = true;
    }
    if ui.button("+ Add extractor").clicked() {
        extractors.push(Extractor {
            source: ExtractorSource::JsonPath { expr: "$.".to_string() },
            var: String::new(),
            scope: ExtractScope::Runtime,
            enabled: true,
        });
        changed = true;
    }
    changed
}

fn scripts_tab(ui: &mut Ui, def: &mut RequestDef, scopes: &VarScopes) -> bool {
    let mut changed = false;
    ui.label("Pre-request script");
    let mut pre = def.scripts.pre_request.clone().unwrap_or_default();
    if code_editor(ui, "script-pre", &mut pre, Lang::Plain, Some(scopes), false, 6, true).changed() {
        def.scripts.pre_request = if pre.is_empty() { None } else { Some(pre) };
        changed = true;
    }
    ui.add_space(6.0);
    ui.label("Post-response script");
    let mut post = def.scripts.post_response.clone().unwrap_or_default();
    if code_editor(ui, "script-post", &mut post, Lang::Plain, Some(scopes), false, 6, true).changed() {
        def.scripts.post_response = if post.is_empty() { None } else { Some(post) };
        changed = true;
    }
    changed
}

fn checkbox_override<T: Copy>(
    ui: &mut Ui,
    label: &str,
    opt: &mut Option<T>,
    default: T,
    changed: &mut bool,
    editor: impl FnOnce(&mut Ui, &mut T, &mut bool),
) {
    ui.horizontal(|ui| {
        let mut enabled = opt.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *opt = if enabled { Some(default) } else { None };
            *changed = true;
        }
        if let Some(v) = opt {
            editor(ui, v, changed);
        }
    });
}

fn settings_tab(ui: &mut Ui, settings: &mut forge_core::model::RequestSettings) -> bool {
    let mut changed = false;
    checkbox_override(ui, "Timeout (ms)", &mut settings.timeout_ms, 30_000, &mut changed, |ui, v, changed| {
        if ui.add(egui::DragValue::new(v).range(1..=600_000)).changed() {
            *changed = true;
        }
    });
    checkbox_override(ui, "Follow redirects", &mut settings.follow_redirects, true, &mut changed, |ui, v, changed| {
        if ui.checkbox(v, "").changed() {
            *changed = true;
        }
    });
    checkbox_override(ui, "Max redirects", &mut settings.max_redirects, 10, &mut changed, |ui, v, changed| {
        if ui.add(egui::DragValue::new(v).range(0..=50)).changed() {
            *changed = true;
        }
    });
    checkbox_override(ui, "Verify TLS certificates", &mut settings.verify_tls, true, &mut changed, |ui, v, changed| {
        if ui.checkbox(v, "").changed() {
            *changed = true;
        }
    });
    if ui.checkbox(&mut settings.skip_in_runs, "Skip in collection runs").changed() {
        changed = true;
    }
    changed
}
