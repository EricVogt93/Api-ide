//! The request editor: method/URL bar, the flat sub-tab strip (Params,
//! Headers, Auth, Body, Assertions, Extract, Scripts, Settings) and the
//! vertical splitter down to the response viewer.

use std::collections::BTreeMap;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, TextBuffer, TextEdit, Ui};

use forge_core::assert::{generate_from_response, GenerateOptions};
use forge_core::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, ExtractScope, Extractor, ExtractorSource, KeyValue,
    Method, MultipartPart, Param, ParamKind, PartContent, RawLanguage, RequestDef, ScriptLang,
};
use forge_core::runner::{RunOptions, RunScope};
use forge_core::store::{TreeNode, Workspace};
use forge_core::vars::{spans, VarScopes};

use crate::bridge::{Bridge, Cmd};
use crate::state::{AppState, RequestSubTab, RunState, StatusMessage, Tab};
use crate::theme::ThemeKind;
use crate::widgets::code_editor::{code_editor, code_editor_numbered, Lang};
use crate::widgets::kv_table::kv_table;
use crate::widgets::method_badge::method_color;
use crate::widgets::response_view::response_view;

/// Render the request editor + response viewer for the active tab.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    let mut send_clicked = false;
    let mut stop_clicked = false;
    let mut save_clicked = false;

    let mut export_def: Option<RequestDef> = None;

    {
        let AppState {
            workspace,
            tabs,
            active_tab,
            active_env,
            theme,
            openapi,
            ..
        } = state;
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
        let spec = openapi.as_ref();

        // Scope every editor widget id (url bar, method combo, code editors,
        // kv tables, ...) below to this tab's `rel_id`, so egui's per-widget
        // state (in particular `TextEdit` undo history) can't leak across
        // tabs: without this, switching tabs and immediately pressing
        // Ctrl+Z could paste the previously active tab's text into this one.
        ui.push_id(egui::Id::new(&tab.rel_id), |ui| {
            render_tab(
                ui,
                tab,
                &scopes,
                theme,
                spec,
                &mut send_clicked,
                &mut stop_clicked,
                &mut save_clicked,
                &mut export_def,
            );
        });
    }

    if save_clicked {
        if let Some(idx) = state.active_tab {
            crate::app::save_tab(state, idx);
        }
    }
    if let Some(def) = export_def {
        state.dialogs.snippet_export.open(def);
    }
    if send_clicked {
        send_active(state, bridge);
    }
    if stop_clicked {
        if let Some(run_id) = state.active_tab_ref().and_then(|t| t.run_id) {
            if let Err(error) = bridge.send(Cmd::Cancel { run_id }) {
                state.status = Some(StatusMessage::error(error));
            }
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
    spec: Option<&forge_core::openapi::ParsedSpec>,
    send_clicked: &mut bool,
    stop_clicked: &mut bool,
    save_clicked: &mut bool,
    export_def: &mut Option<RequestDef>,
) {
    // One code path for both orientations: `total` is the split axis (width
    // when side-by-side, height when stacked), `cross` the fixed axis. A pane
    // dragged thinner than COLLAPSE snaps to a thin restore strip.
    let seam = ui.visuals().widgets.noninteractive.bg_stroke;
    let horizontal = tab.split_horizontal;
    let total = if horizontal {
        ui.available_width()
    } else {
        ui.available_height()
    };
    let cross = if horizontal {
        ui.available_height()
    } else {
        ui.available_width()
    };
    const SP: f32 = 8.0;
    const COLLAPSE: f32 = 40.0;
    const STRIP: f32 = 24.0;
    let first = (total * tab.split_ratio).clamp(0.0, total);
    let second = (total - first - SP).max(0.0);
    let vm = |m: f32| {
        if horizontal {
            egui::vec2(m.max(0.0), cross)
        } else {
            egui::vec2(cross, m.max(0.0))
        }
    };
    let td = egui::Layout::top_down(egui::Align::Min);

    let mut body = |ui: &mut Ui| {
        let avail = |ui: &Ui| {
            if horizontal {
                ui.available_width()
            } else {
                ui.available_height()
            }
        };
        if first < COLLAPSE {
            // Request collapsed to a strip; response fills the rest.
            if expand_strip(ui, vm(STRIP), "request", horizontal) {
                tab.split_ratio = 0.5;
            }
            ui.allocate_ui_with_layout(vm(avail(ui)), td, |ui| {
                render_response_pane(ui, tab, theme);
            });
        } else if second < COLLAPSE {
            // Response collapsed to a strip; request fills the rest.
            let req = (avail(ui) - STRIP).max(COLLAPSE);
            ui.allocate_ui_with_layout(vm(req), td, |ui| {
                render_request_pane(
                    ui,
                    tab,
                    scopes,
                    theme,
                    spec,
                    send_clicked,
                    stop_clicked,
                    save_clicked,
                    export_def,
                );
            });
            if expand_strip(ui, vm(avail(ui)), "response", horizontal) {
                tab.split_ratio = 0.5;
            }
        } else {
            ui.allocate_ui_with_layout(vm(first), td, |ui| {
                render_request_pane(
                    ui,
                    tab,
                    scopes,
                    theme,
                    spec,
                    send_clicked,
                    stop_clicked,
                    save_clicked,
                    export_def,
                );
            });
            let sp = ui.allocate_response(vm(SP), egui::Sense::drag());
            let active = sp.hovered() || sp.dragged();
            let line = if active {
                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill)
            } else {
                seam
            };
            if horizontal {
                ui.painter()
                    .vline(sp.rect.center().x, sp.rect.y_range(), line);
            } else {
                ui.painter()
                    .hline(sp.rect.x_range(), sp.rect.center().y, line);
            }
            if sp.dragged() && total > 1.0 {
                let d = if horizontal {
                    sp.drag_delta().x
                } else {
                    sp.drag_delta().y
                };
                tab.split_ratio = ((first + d) / total).clamp(0.0, 1.0);
            }
            if sp.hovered() || sp.dragged() {
                ui.ctx().set_cursor_icon(if horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
            }
            ui.allocate_ui_with_layout(vm(avail(ui)), td, |ui| {
                render_response_pane(ui, tab, theme);
            });
        }
    };

    if horizontal {
        ui.horizontal_top(body);
    } else {
        body(ui);
    }
}

/// Byte index where the path part of a request URL starts: after a leading
/// `{{variable}}`, or after `scheme://host`, else 0 (bare path).
fn url_path_start(url: &str) -> usize {
    if let Some(rest) = url.strip_prefix("{{") {
        if let Some(e) = rest.find("}}") {
            return e + 4;
        }
    }
    if let Some(p) = url.find("://") {
        let after = &url[p + 3..];
        return match after.find('/') {
            Some(i) => p + 3 + i,
            None => url.len(),
        };
    }
    0
}

/// OpenAPI assistance under the URL bar: an inline inspection line when the
/// URL/method doesn't match the spec, and a suggestion popup (matching spec
/// operations) while the URL field has focus.
#[allow(clippy::too_many_arguments)]
fn openapi_url_assist(
    ui: &mut Ui,
    tab: &mut Tab,
    spec: &forge_core::openapi::ParsedSpec,
    theme: ThemeKind,
    matched: bool,
    url_rect: egui::Rect,
    url_focused: bool,
) {
    // Inspection line (IDE-style annotation) under the request bar.
    if !matched && !tab.def.url.trim().is_empty() {
        let warn = theme.warn_color();
        let msg = if spec.any_path_matches(&tab.def.url) {
            let allowed: Vec<&str> = spec
                .operations
                .iter()
                .filter(|op| {
                    forge_core::openapi::path_matches_template(
                        &op.path,
                        &forge_core::openapi::url_to_path(&tab.def.url),
                    )
                })
                .map(|op| op.method.as_str())
                .collect();
            format!(
                "{} {} not allowed by spec here (allowed: {})",
                crate::theme::icons::WARNING,
                tab.def.method.as_str(),
                allowed.join(", ")
            )
        } else {
            format!(
                "{} Path not found in OpenAPI spec \"{}\"",
                crate::theme::icons::WARNING,
                spec.title
            )
        };
        ui.label(RichText::new(msg).size(13.0).color(warn));
    }

    // Suggestion popup while typing in the URL field.
    if !url_focused {
        return;
    }
    let suggestions: Vec<(Method, String, String)> = spec
        .suggest(&tab.def.url)
        .into_iter()
        .take(8)
        .map(|op| (op.method, op.path.clone(), op.summary.clone()))
        .collect();
    if suggestions.is_empty() {
        return;
    }
    let panel_bg = ui.visuals().panel_fill;
    let seam = ui.visuals().widgets.noninteractive.bg_stroke;
    egui::Area::new(ui.id().with("openapi-suggest"))
        .fixed_pos(url_rect.left_bottom() + egui::vec2(0.0, 4.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(panel_bg)
                .stroke(seam)
                .corner_radius(6u8)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_width(url_rect.width().min(560.0));
                    for (method, path, summary) in suggestions {
                        let row = ui
                            .horizontal(|ui| {
                                ui.label(
                                    RichText::new(method.as_str())
                                        .color(method_color(method))
                                        .monospace()
                                        .strong()
                                        .size(12.0),
                                );
                                ui.label(RichText::new(&path).monospace().size(14.0));
                                if !summary.is_empty() {
                                    ui.label(
                                        RichText::new(&summary)
                                            .size(13.0)
                                            .color(ui.visuals().weak_text_color()),
                                    );
                                }
                            })
                            .response;
                        let row = ui.interact(
                            row.rect.expand2(egui::vec2(4.0, 1.0)),
                            ui.id().with(("sugg", &path, method.as_str())),
                            egui::Sense::click(),
                        );
                        if row.hovered() {
                            ui.painter().rect_filled(
                                row.rect,
                                4u8,
                                ui.visuals().widgets.hovered.bg_fill,
                            );
                        }
                        if row.clicked() {
                            let keep = url_path_start(&tab.def.url);
                            tab.def.url = format!("{}{}", &tab.def.url[..keep], path);
                            tab.def.method = method;
                            tab.dirty = true;
                        }
                    }
                });
        });
}

/// One-line hint listing the spec's required query parameters missing from
/// the request, with an "Add" action appending them. Returns `true` when it
/// changed the params.
fn missing_params_hint(
    ui: &mut Ui,
    op: &forge_core::openapi::SpecOperation,
    theme: ThemeKind,
    params: &mut Vec<Param>,
) -> bool {
    let missing: Vec<&str> = op
        .query_params
        .iter()
        .filter(|(name, required)| {
            *required && !params.iter().any(|p| p.kv.key.eq_ignore_ascii_case(name))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if missing.is_empty() {
        return false;
    }
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} Missing required: {}",
                crate::theme::icons::WARNING,
                missing.join(", ")
            ))
            .size(13.0)
            .color(theme.warn_color()),
        );
        if ui.small_button("Add").clicked() {
            for name in &missing {
                params.push(Param {
                    kv: KeyValue::new(*name, ""),
                    kind: ParamKind::Query,
                });
            }
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::AuthKind;

    #[test]
    fn unavailable_oauth_authorization_code_is_not_offered() {
        assert!(!AuthKind::ALL.contains(&AuthKind::OAuth2AuthCode));
    }
}

/// A thin clickable strip shown in place of a collapsed pane. Returns `true`
/// when clicked (the caller restores the split). `label` names the hidden
/// pane; `horizontal` picks a vertical strip (side split) vs a horizontal one.
fn expand_strip(ui: &mut Ui, size: egui::Vec2, label: &str, horizontal: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_text(format!("Show {label}"));
    let bg = if resp.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().panel_fill
    };
    ui.painter().rect_filled(rect, 0.0, bg);
    let dim = ui.visuals().weak_text_color();
    if horizontal {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            crate::theme::icons::TRIANGLE_RIGHT,
            egui::FontId::proportional(14.0),
            dim,
        );
    } else {
        ui.painter().text(
            rect.left_center() + egui::vec2(4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!(
                "{}  {}",
                crate::theme::icons::TRIANGLE_RIGHT,
                label.to_uppercase()
            ),
            egui::FontId::proportional(13.0),
            dim,
        );
    }
    resp.clicked()
}

#[allow(clippy::too_many_arguments)]
fn render_request_pane(
    ui: &mut Ui,
    tab: &mut Tab,
    scopes: &VarScopes,
    theme: ThemeKind,
    spec: Option<&forge_core::openapi::ParsedSpec>,
    send_clicked: &mut bool,
    stop_clicked: &mut bool,
    save_clicked: &mut bool,
    export_def: &mut Option<RequestDef>,
) {
    egui::ScrollArea::vertical()
        .id_salt("request-editor-scroll")
        // Fill the allocated height so the splitter sits at the pane boundary
        // (with default auto-shrink it hugged the content, so the draggable
        // seam no longer matched `first` and dragging felt dead).
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let dark = theme.editor_bg().r() < 128;

            let elev = ui.visuals().widgets.hovered.bg_fill;
            let border = egui::Color32::from_rgb(0x4A, 0x4D, 0x53);
            let mut url_field: Option<(egui::Rect, bool)> = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut method = tab.def.method;
                egui::ComboBox::from_id_salt("method-combo")
                    .selected_text(
                        RichText::new(method.as_str())
                            .color(method_color(method))
                            .strong(),
                    )
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        for m in Method::ALL {
                            ui.selectable_value(&mut method, m, m.as_str());
                        }
                    });
                if method != tab.def.method {
                    tab.def.method = method;
                    tab.dirty = true;
                }

                // Send + save + code cluster is laid out right-to-left so the
                // URL box (added after) flexes to fill the gap between.
                let mut send = false;
                let mut stop = false;
                let mut save = false;
                let mut export = false;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(crate::theme::icons::CODE)
                        .on_hover_text("Export code\u{2026}")
                        .clicked()
                    {
                        export = true;
                    }
                    if ui
                        .button(crate::theme::icons::SAVE)
                        .on_hover_text("Save")
                        .clicked()
                    {
                        save = true;
                    }
                    if tab.run_id.is_some() {
                        let b = egui::Button::new(
                            RichText::new(format!("{}  Stop", crate::theme::icons::STOP))
                                .color(egui::Color32::WHITE),
                        )
                        .fill(theme.error_color());
                        if ui.add(b).clicked() {
                            stop = true;
                        }
                        ui.spinner();
                    } else {
                        let b = egui::Button::new(
                            RichText::new(format!("{}  Send", crate::theme::icons::PLAY))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(theme.accent_color());
                        if ui.add(b).clicked() {
                            send = true;
                        }
                    }

                    // URL field as an elevated rounded box filling the space
                    // between the method combo and the button cluster. The box
                    // is the TextEdit's own frame, restyled via visuals.
                    {
                        let v = ui.visuals_mut();
                        v.extreme_bg_color = elev;
                        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
                        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, border);
                        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, theme.accent_color());
                    }
                    let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                        let mut job = url_layout_job(buf.as_str(), scopes, dark);
                        job.wrap.max_width = wrap_width;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let resp = ui.add(
                        TextEdit::singleline(&mut tab.def.url)
                            .id_salt("url-bar")
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(10, 8))
                            .font(egui::FontSelection::from(FontId::monospace(15.0)))
                            .layouter(&mut layouter),
                    );
                    if resp.changed() {
                        tab.dirty = true;
                    }
                    url_field = Some((resp.rect, resp.has_focus()));
                });
                *send_clicked |= send;
                *stop_clicked |= stop;
                *save_clicked |= save;
                if export {
                    *export_def = Some(tab.def.clone());
                }
            });

            // OpenAPI editor assistance: inline annotation + suggestions.
            let spec_op = spec.and_then(|s| s.find_operation(tab.def.method, &tab.def.url));
            if let (Some(spec), Some((rect, focused))) = (spec, url_field) {
                openapi_url_assist(ui, tab, spec, theme, spec_op.is_some(), rect, focused);
            }

            ui.add_space(6.0);
            request_sub_tabs(ui, tab, elev);
            ui.add_space(6.0);

            let changed = match tab.sub_tab {
                // "Request": params, headers and auth stacked as sections in
                // one scrollable tab (one aspect per tab was too many tabs).
                RequestSubTab::Params => {
                    let mut c = false;
                    section(ui, "Query Parameters");
                    if let Some(op) = spec_op {
                        c |= missing_params_hint(ui, op, theme, &mut tab.def.params);
                    }
                    c |= params_tab(ui, &mut tab.def.params);
                    ui.add_space(16.0);
                    section(ui, "Headers");
                    c |= kv_grid(ui, "req-headers", &mut tab.def.headers, "Add header");
                    ui.add_space(16.0);
                    section(ui, "Authorization");
                    c |= auth_tab(ui, &mut tab.def.auth);
                    c
                }
                RequestSubTab::Body => body_tab(ui, &mut tab.def.body, scopes),
                // "Tests": assertions, extractors and scripts stacked.
                RequestSubTab::Assertions => {
                    let mut c = false;
                    section(ui, "Assertions");
                    c |= assertions_tab(ui, &mut tab.def, tab.response.as_ref());
                    ui.add_space(16.0);
                    section(ui, "Extract");
                    c |= extract_tab(ui, &mut tab.def.extractors);
                    ui.add_space(16.0);
                    section(ui, "Scripts");
                    c |= scripts_tab(ui, &mut tab.def, scopes);
                    c
                }
                RequestSubTab::Settings => settings_tab(ui, &mut tab.def.settings),
            };
            if changed {
                tab.dirty = true;
            }
        });
}

fn render_response_pane(ui: &mut Ui, tab: &mut Tab, theme: ThemeKind) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("RESPONSE")
                .size(12.0)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (icon, tip) = if tab.split_horizontal {
                (crate::theme::icons::SPLIT_STACKED, "Stack response below")
            } else {
                (crate::theme::icons::SPLIT_SIDE, "Response to the side")
            };
            if ui.button(icon).on_hover_text(tip).clicked() {
                tab.split_horizontal = !tab.split_horizontal;
            }
        });
    });
    ui.add_space(3.0);
    ui.allocate_ui(
        egui::vec2(ui.available_width(), ui.available_height()),
        |ui| {
            response_view(ui, tab.response.as_ref(), &mut tab.response_state, theme);
        },
    );
}

pub fn send_active(state: &mut AppState, bridge: &Bridge) {
    let Some(idx) = state.active_tab else { return };
    let Some(workspace) = state.workspace.clone() else {
        state.status = Some(StatusMessage::error("No workspace open"));
        return;
    };
    let rel_id = state.tabs[idx].rel_id.clone();
    let scope = RunScope::Request(rel_id);
    let options = RunOptions {
        environment: state.active_env.clone(),
        ..Default::default()
    };
    state.last_run = Some((scope.clone(), options.clone()));
    let run_id = state.alloc_run_id();
    state.tabs[idx].run_id = Some(run_id);
    state.run_state = RunState {
        run_id: Some(run_id),
        total: 1,
        completed: 0,
    };
    state.run_log.start(run_id);
    if let Err(error) = bridge.send(Cmd::Run {
        run_id,
        workspace: Box::new(workspace),
        scope,
        options,
    }) {
        state.tabs[idx].run_id = None;
        state.run_state = RunState::default();
        state.status = Some(StatusMessage::error(error));
    }
}

/// Build a best-effort variable scope for editor highlighting: the active
/// environment plus the collection/folder chain that owns `rel_id`. This is
/// a preview aid only — the authoritative resolution happens in
/// `forge_core::runner` when the request actually executes.
pub(super) fn build_scopes(
    workspace: &Workspace,
    rel_id: &str,
    active_env: Option<&str>,
) -> VarScopes {
    let mut scopes = VarScopes::new();
    if let Some(env_name) = active_env {
        if let Some(loaded) = workspace.environment(env_name) {
            scopes = scopes.with_environment(&loaded.env, &loaded.secrets);
        }
    }
    for col in &workspace.collections {
        if let Some(folder_vars) = find_ancestor_vars(&col.children, rel_id, workspace) {
            scopes = scopes
                .with_collection(&col.meta.variables)
                .with_folders(folder_vars.iter().copied());
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
    // Relay dims the scheme + host and brightens the path/query.
    let (host_c, path_c, var_fg) = if dark {
        (
            Color32::from_rgb(0x86, 0x8A, 0x91),
            Color32::from_rgb(0xDF, 0xE1, 0xE5),
            Color32::from_rgb(0xFF, 0xC6, 0x6D),
        )
    } else {
        (
            Color32::from_rgb(0x6C, 0x70, 0x7E),
            Color32::from_rgb(0x27, 0x28, 0x2E),
            Color32::from_rgb(0xB3, 0x6B, 0x00),
        )
    };
    // End of the host: the first `/` after `://`, else the first `/`.
    let host_end = match text.find("://") {
        Some(p) => text[p + 3..]
            .find('/')
            .map(|i| p + 3 + i)
            .unwrap_or(text.len()),
        None => text.find('/').unwrap_or(text.len()),
    };
    let font = FontId::monospace(15.0);
    let mut job = LayoutJob::default();
    // Append a non-variable run [start,end), split at host_end into host/path.
    let append_base = |job: &mut LayoutJob, start: usize, end: usize| {
        if start >= end {
            return;
        }
        if end <= host_end {
            job.append(
                &text[start..end],
                0.0,
                TextFormat::simple(font.clone(), host_c),
            );
        } else if start >= host_end {
            job.append(
                &text[start..end],
                0.0,
                TextFormat::simple(font.clone(), path_c),
            );
        } else {
            job.append(
                &text[start..host_end],
                0.0,
                TextFormat::simple(font.clone(), host_c),
            );
            job.append(
                &text[host_end..end],
                0.0,
                TextFormat::simple(font.clone(), path_c),
            );
        }
    };
    let mut cursor = 0usize;
    for v in &spans(text, scopes) {
        append_base(&mut job, cursor, v.start);
        let mut fmt = TextFormat::simple(font.clone(), var_fg);
        fmt.background = Color32::from_rgba_unmultiplied(var_fg.r(), var_fg.g(), var_fg.b(), 30);
        job.append(&text[v.start..v.end], 0.0, fmt);
        cursor = v.end;
    }
    append_base(&mut job, cursor, text.len());
    job
}

/// A dim, uppercase section subheading inside a combined request tab.
fn section(ui: &mut Ui, title: &str) {
    ui.label(
        RichText::new(title.to_uppercase())
            .size(12.0)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(4.0);
}

/// Request sub-tab strip: four consolidated tabs (Request / Body / Tests /
/// Settings) with a count pill and a 2px accent underline under the active.
fn request_sub_tabs(ui: &mut Ui, tab: &mut Tab, elev: egui::Color32) {
    let params_headers = tab
        .def
        .params
        .iter()
        .filter(|p| !p.kv.key.is_empty())
        .count()
        + tab.def.headers.iter().filter(|h| !h.key.is_empty()).count();
    let tests = tab.def.assertions.len() + tab.def.extractors.len();
    let items: [(RequestSubTab, &str, usize); 4] = [
        (RequestSubTab::Params, "Request", params_headers),
        (RequestSubTab::Body, "Body", 0),
        (RequestSubTab::Assertions, "Tests", tests),
        (RequestSubTab::Settings, "Settings", 0),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (val, label, count) in items {
            let selected = tab.sub_tab == val;
            let color = if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().weak_text_color()
            };
            let resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let r = ui.add(
                        egui::Label::new(RichText::new(label).color(color).strong())
                            .sense(egui::Sense::click()),
                    );
                    if count > 0 {
                        egui::Frame::NONE
                            .fill(elev)
                            .corner_radius(8u8)
                            .inner_margin(egui::Margin::symmetric(6, 1))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(count.to_string())
                                        .size(12.0)
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                );
                            });
                    }
                    r
                })
                .inner;
            if selected {
                let rect = resp.rect;
                let y = rect.bottom() + 4.0;
                ui.painter().line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(2.0, color),
                );
            }
            if resp.clicked() {
                tab.sub_tab = val;
            }
        }
    });
    ui.add_space(2.0);
    ui.separator();
}

fn params_tab(ui: &mut Ui, params: &mut Vec<Param>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;

    let hcol = |ui: &mut Ui, t: &str| {
        ui.label(
            RichText::new(t)
                .size(12.0)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
    };
    // Relay's params table is KEY/VALUE/DESCRIPTION only. The Query/Path kind
    // lives in a right-click menu on each row; the TYPE column only appears
    // once a Path param exists (so it stays discoverable without cluttering
    // the common case).
    let show_type = params.iter().any(|p| p.kind == ParamKind::Path);
    let ncols = if show_type { 6 } else { 5 };
    if !params.is_empty() {
        egui::Grid::new("params-grid")
            .num_columns(ncols)
            .spacing(egui::vec2(10.0, 8.0))
            .show(ui, |ui| {
                hcol(ui, "");
                hcol(ui, "KEY");
                hcol(ui, "VALUE");
                if show_type {
                    hcol(ui, "TYPE");
                }
                hcol(ui, "DESCRIPTION");
                hcol(ui, "");
                ui.end_row();

                #[allow(clippy::needless_range_loop)]
                for i in 0..params.len() {
                    if ui.checkbox(&mut params[i].kv.enabled, "").changed() {
                        changed = true;
                    }
                    let mut kind = params[i].kind;
                    let key_resp =
                        ui.add(TextEdit::singleline(&mut params[i].kv.key).desired_width(160.0));
                    if key_resp.changed() {
                        changed = true;
                    }
                    key_resp.context_menu(|ui| {
                        if ui
                            .selectable_label(kind == ParamKind::Query, "Query param")
                            .clicked()
                        {
                            kind = ParamKind::Query;
                            ui.close();
                        }
                        if ui
                            .selectable_label(kind == ParamKind::Path, "Path param")
                            .clicked()
                        {
                            kind = ParamKind::Path;
                            ui.close();
                        }
                    });
                    if ui
                        .add(TextEdit::singleline(&mut params[i].kv.value).desired_width(200.0))
                        .changed()
                    {
                        changed = true;
                    }
                    if show_type {
                        egui::ComboBox::from_id_salt(("param-kind", i))
                            .selected_text(if kind == ParamKind::Query {
                                "Query"
                            } else {
                                "Path"
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut kind, ParamKind::Query, "Query");
                                ui.selectable_value(&mut kind, ParamKind::Path, "Path");
                            });
                    }
                    if kind != params[i].kind {
                        params[i].kind = kind;
                        changed = true;
                    }
                    if ui
                        .add(
                            TextEdit::singleline(&mut params[i].kv.description)
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    let dim = ui.visuals().weak_text_color();
                    let x = ui.add(
                        egui::Label::new(RichText::new(crate::theme::icons::CLOSE).color(dim))
                            .sense(egui::Sense::click()),
                    );
                    if x.on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
    }

    ui.add_space(6.0);
    if add_row_link(ui, "Add parameter") {
        params.push(Param {
            kv: KeyValue::new("", ""),
            kind: ParamKind::Query,
        });
        changed = true;
    }

    if let Some(i) = remove {
        params.remove(i);
        changed = true;
    }
    changed
}

/// A dim "＋ <label>" clickable link (Relay's "Add parameter" row affordance).
fn add_row_link(ui: &mut Ui, label: &str) -> bool {
    let dim = ui.visuals().weak_text_color();
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(format!("{}  {label}", crate::theme::icons::ADD)).color(dim),
            )
            .sense(egui::Sense::click()),
        )
        .clicked()
    })
    .inner
}

/// A clean key/value editor grid (KEY / VALUE / DESCRIPTION) matching the
/// params table, with a subtle per-row remove and an "＋ add" link. Used for
/// headers and any other `KeyValue` list. Returns whether anything changed.
fn kv_grid(ui: &mut Ui, id_salt: &str, rows: &mut Vec<KeyValue>, add_label: &str) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let hcol = |ui: &mut Ui, t: &str| {
        ui.label(
            RichText::new(t)
                .size(12.0)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
    };
    if !rows.is_empty() {
        egui::Grid::new(id_salt)
            .num_columns(5)
            .spacing(egui::vec2(10.0, 8.0))
            .show(ui, |ui| {
                hcol(ui, "");
                hcol(ui, "KEY");
                hcol(ui, "VALUE");
                hcol(ui, "DESCRIPTION");
                hcol(ui, "");
                ui.end_row();

                #[allow(clippy::needless_range_loop)]
                for i in 0..rows.len() {
                    if ui.checkbox(&mut rows[i].enabled, "").changed() {
                        changed = true;
                    }
                    if ui
                        .add(TextEdit::singleline(&mut rows[i].key).desired_width(160.0))
                        .changed()
                    {
                        changed = true;
                    }
                    if ui
                        .add(TextEdit::singleline(&mut rows[i].value).desired_width(200.0))
                        .changed()
                    {
                        changed = true;
                    }
                    if ui
                        .add(TextEdit::singleline(&mut rows[i].description).desired_width(220.0))
                        .changed()
                    {
                        changed = true;
                    }
                    let dim = ui.visuals().weak_text_color();
                    if ui
                        .add(
                            egui::Label::new(RichText::new(crate::theme::icons::CLOSE).color(dim))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
    }

    ui.add_space(6.0);
    if add_row_link(ui, add_label) {
        rows.push(KeyValue::new("", ""));
        changed = true;
    }
    if let Some(i) = remove {
        rows.remove(i);
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
    Digest,
    Ntlm,
    AwsSigV4,
    OAuth2ClientCredentials,
    OAuth2AuthCode,
}

impl AuthKind {
    const ALL: [AuthKind; 9] = [
        AuthKind::Inherit,
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::ApiKey,
        AuthKind::Digest,
        AuthKind::Ntlm,
        AuthKind::AwsSigV4,
        AuthKind::OAuth2ClientCredentials,
    ];

    fn label(&self) -> &'static str {
        match self {
            AuthKind::None => "None",
            AuthKind::Inherit => "Inherit",
            AuthKind::Basic => "Basic",
            AuthKind::Bearer => "Bearer Token",
            AuthKind::ApiKey => "API Key",
            AuthKind::Digest => "Digest",
            AuthKind::Ntlm => "NTLM",
            AuthKind::AwsSigV4 => "AWS Signature v4",
            AuthKind::OAuth2ClientCredentials => "OAuth 2.0 (Client Credentials)",
            AuthKind::OAuth2AuthCode => "OAuth 2.0 (Authorization Code — unavailable)",
        }
    }

    fn of(auth: &AuthConfig) -> Self {
        match auth {
            AuthConfig::None => AuthKind::None,
            AuthConfig::Inherit => AuthKind::Inherit,
            AuthConfig::Basic { .. } => AuthKind::Basic,
            AuthConfig::Bearer { .. } => AuthKind::Bearer,
            AuthConfig::ApiKey { .. } => AuthKind::ApiKey,
            AuthConfig::Digest { .. } => AuthKind::Digest,
            AuthConfig::Ntlm { .. } => AuthKind::Ntlm,
            AuthConfig::AwsSigV4 { .. } => AuthKind::AwsSigV4,
            AuthConfig::OAuth2ClientCredentials { .. } => AuthKind::OAuth2ClientCredentials,
            AuthConfig::OAuth2AuthCode { .. } => AuthKind::OAuth2AuthCode,
        }
    }

    fn default_config(&self) -> AuthConfig {
        match self {
            AuthKind::None => AuthConfig::None,
            AuthKind::Inherit => AuthConfig::Inherit,
            AuthKind::Basic => AuthConfig::Basic {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::Bearer => AuthConfig::Bearer {
                token: String::new(),
                prefix: None,
            },
            AuthKind::ApiKey => AuthConfig::ApiKey {
                key: String::new(),
                value: String::new(),
                placement: ApiKeyPlacement::Header,
            },
            AuthKind::Digest => AuthConfig::Digest {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::Ntlm => AuthConfig::Ntlm {
                username: String::new(),
                password: String::new(),
                domain: String::new(),
            },
            AuthKind::AwsSigV4 => AuthConfig::AwsSigV4 {
                access_key: String::new(),
                secret_key: String::new(),
                session_token: None,
                region: String::new(),
                service: String::new(),
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
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new(label),
        );
        add(ui);
    });
}

fn auth_tab(ui: &mut Ui, auth: &mut AuthConfig) -> bool {
    let mut changed = false;
    let mut kind = AuthKind::of(auth);
    let prev = kind;
    egui::ComboBox::from_id_salt("auth-kind")
        .selected_text(kind.label())
        .show_ui(ui, |ui| {
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
            field(ui, "Username", |ui| {
                changed |= ui.text_edit_singleline(username).changed()
            });
            field(ui, "Password", |ui| {
                changed |= ui
                    .add(TextEdit::singleline(password).password(true))
                    .changed()
            });
        }
        AuthConfig::Bearer { token, prefix } => {
            field(ui, "Token", |ui| {
                changed |= ui.add(TextEdit::singleline(token).password(true)).changed()
            });
            let mut p = prefix.clone().unwrap_or_else(|| "Bearer".to_string());
            field(ui, "Prefix", |ui| {
                if ui.text_edit_singleline(&mut p).changed() {
                    *prefix = if p.is_empty() || p == "Bearer" {
                        None
                    } else {
                        Some(p.clone())
                    };
                    changed = true;
                }
            });
        }
        AuthConfig::Digest { username, password } => {
            field(ui, "Username", |ui| {
                changed |= ui.text_edit_singleline(username).changed()
            });
            field(ui, "Password", |ui| {
                changed |= ui
                    .add(TextEdit::singleline(password).password(true))
                    .changed()
            });
            ui.weak("Answers the server's 401 Digest challenge automatically (RFC 7616).");
        }
        AuthConfig::Ntlm {
            username,
            password,
            domain,
        } => {
            field(ui, "Username", |ui| {
                changed |= ui.text_edit_singleline(username).changed()
            });
            field(ui, "Password", |ui| {
                changed |= ui
                    .add(TextEdit::singleline(password).password(true))
                    .changed()
            });
            field(ui, "Domain", |ui| {
                changed |= ui.text_edit_singleline(domain).changed()
            });
            ui.weak("Runs the NTLMv2 Negotiate/Challenge/Authenticate handshake on 401.");
        }
        AuthConfig::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            field(ui, "Access key", |ui| {
                changed |= ui.text_edit_singleline(access_key).changed()
            });
            field(ui, "Secret key", |ui| {
                changed |= ui
                    .add(TextEdit::singleline(secret_key).password(true))
                    .changed()
            });
            let mut token = session_token.clone().unwrap_or_default();
            field(ui, "Session token", |ui| {
                if ui
                    .add(TextEdit::singleline(&mut token).password(true))
                    .changed()
                {
                    *session_token = if token.is_empty() {
                        None
                    } else {
                        Some(token.clone())
                    };
                    changed = true;
                }
            });
            field(ui, "Region", |ui| {
                changed |= ui.text_edit_singleline(region).changed()
            });
            field(ui, "Service", |ui| {
                changed |= ui.text_edit_singleline(service).changed()
            });
        }
        AuthConfig::ApiKey {
            key,
            value,
            placement,
        } => {
            field(ui, "Key", |ui| {
                changed |= ui.text_edit_singleline(key).changed()
            });
            field(ui, "Value", |ui| {
                changed |= ui.add(TextEdit::singleline(value).password(true)).changed()
            });
            field(ui, "Add to", |ui| {
                egui::ComboBox::from_id_salt("apikey-placement")
                    .selected_text(if *placement == ApiKeyPlacement::Header {
                        "Header"
                    } else {
                        "Query"
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(placement, ApiKeyPlacement::Header, "Header")
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .selectable_value(placement, ApiKeyPlacement::Query, "Query")
                            .changed()
                        {
                            changed = true;
                        }
                    });
            });
        }
        AuthConfig::OAuth2ClientCredentials {
            token_url,
            client_id,
            client_secret,
            credentials_in_body,
            ..
        } => {
            field(ui, "Token URL", |ui| {
                changed |= ui.text_edit_singleline(token_url).changed()
            });
            field(ui, "Client ID", |ui| {
                changed |= ui.text_edit_singleline(client_id).changed()
            });
            field(ui, "Client Secret", |ui| {
                changed |= ui
                    .add(TextEdit::singleline(client_secret).password(true))
                    .changed()
            });
            field(ui, "Credentials in body", |ui| {
                changed |= ui.checkbox(credentials_in_body, "").changed()
            });
        }
        AuthConfig::OAuth2AuthCode {
            auth_url,
            token_url,
            client_id,
            client_secret,
            pkce,
            ..
        } => {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Authorization Code is not executable in this build. Choose another auth method.",
            );
            field(ui, "Auth URL", |ui| {
                changed |= ui.text_edit_singleline(auth_url).changed()
            });
            field(ui, "Token URL", |ui| {
                changed |= ui.text_edit_singleline(token_url).changed()
            });
            field(ui, "Client ID", |ui| {
                changed |= ui.text_edit_singleline(client_id).changed()
            });
            let mut secret = client_secret.clone().unwrap_or_default();
            field(ui, "Client Secret", |ui| {
                if ui
                    .add(TextEdit::singleline(&mut secret).password(true))
                    .changed()
                {
                    *client_secret = if secret.is_empty() {
                        None
                    } else {
                        Some(secret.clone())
                    };
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
            BodyKind::Json => BodyDef::Json {
                text: String::new(),
            },
            BodyKind::Xml => BodyDef::Xml {
                text: String::new(),
            },
            BodyKind::Raw => BodyDef::Raw {
                text: String::new(),
                language: RawLanguage::Text,
            },
            BodyKind::Form => BodyDef::FormUrlencoded { fields: Vec::new() },
            BodyKind::Multipart => BodyDef::Multipart { parts: Vec::new() },
            BodyKind::GraphQl => BodyDef::GraphQl {
                query: String::new(),
                variables: String::new(),
                operation_name: None,
            },
            BodyKind::Binary => BodyDef::Binary {
                path: String::new(),
            },
        }
    }
}

/// Pretty-print a JSON string (2-space indent). Returns `None` if the text
/// is not valid JSON (e.g. it still contains bare `{{variables}}`).
fn beautify_json(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

fn body_tab(ui: &mut Ui, body: &mut BodyDef, scopes: &VarScopes) -> bool {
    let mut changed = false;
    let mut kind = BodyKind::of(body);
    let prev = kind;
    let mut beautify = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("body-kind")
            .selected_text(kind.label())
            .show_ui(ui, |ui| {
                for k in BodyKind::ALL {
                    ui.selectable_value(&mut kind, k, k.label());
                }
            });
        // Beautify (pretty-print) for JSON bodies, including raw-json.
        let is_json = matches!(kind, BodyKind::Json)
            || matches!(
                body,
                BodyDef::Raw {
                    language: RawLanguage::Json,
                    ..
                }
            );
        if is_json {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Beautify")
                    .on_hover_text("Pretty-print JSON")
                    .clicked()
                {
                    beautify = true;
                }
            });
        }
    });
    if kind != prev {
        *body = kind.default_body();
        changed = true;
    }
    if beautify {
        let target = match body {
            BodyDef::Json { text } => Some(text),
            BodyDef::Raw {
                text,
                language: RawLanguage::Json,
            } => Some(text),
            _ => None,
        };
        if let Some(text) = target {
            if let Some(pretty) = beautify_json(text) {
                *text = pretty;
                changed = true;
            }
        }
    }
    ui.add_space(4.0);

    match body {
        BodyDef::None => {
            ui.weak("This request has no body.");
        }
        BodyDef::Json { text } => {
            if code_editor_numbered(
                ui,
                "body-json",
                text,
                Lang::Json,
                Some(scopes),
                false,
                10,
                false,
            )
            .changed()
            {
                changed = true;
            }
        }
        BodyDef::Xml { text } => {
            if code_editor_numbered(
                ui,
                "body-xml",
                text,
                Lang::Xml,
                Some(scopes),
                false,
                10,
                false,
            )
            .changed()
            {
                changed = true;
            }
        }
        BodyDef::Raw { text, language } => {
            let mut lang = *language;
            egui::ComboBox::from_id_salt("raw-lang")
                .selected_text(format!("{lang:?}"))
                .show_ui(ui, |ui| {
                    for l in [
                        RawLanguage::Text,
                        RawLanguage::Json,
                        RawLanguage::Xml,
                        RawLanguage::Html,
                        RawLanguage::Yaml,
                    ] {
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
            if code_editor_numbered(
                ui,
                "body-raw",
                text,
                editor_lang,
                Some(scopes),
                false,
                10,
                false,
            )
            .changed()
            {
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
        BodyDef::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            ui.label("Query");
            if code_editor(
                ui,
                "gql-query",
                query,
                Lang::GraphQl,
                Some(scopes),
                false,
                8,
                true,
            )
            .changed()
            {
                changed = true;
            }
            ui.label("Variables (JSON)");
            if code_editor(
                ui,
                "gql-vars",
                variables,
                Lang::Json,
                Some(scopes),
                false,
                4,
                true,
            )
            .changed()
            {
                changed = true;
            }
            let mut op = operation_name.clone().unwrap_or_default();
            field(ui, "Operation name", |ui| {
                if ui.text_edit_singleline(&mut op).changed() {
                    *operation_name = if op.is_empty() {
                        None
                    } else {
                        Some(op.clone())
                    };
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
                part.content = PartContent::Text {
                    value: String::new(),
                };
                changed = true;
            }
            if ui.selectable_label(is_file, "File").clicked() && !is_file {
                part.content = PartContent::File {
                    path: String::new(),
                };
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
        parts.push(MultipartPart {
            name: String::new(),
            content: PartContent::Text {
                value: String::new(),
            },
            content_type: None,
            enabled: true,
        });
        changed = true;
    }
    changed
}

fn assertions_tab(
    ui: &mut Ui,
    def: &mut RequestDef,
    response: Option<&forge_core::runner::RequestOutcome>,
) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;

    egui::ScrollArea::vertical()
        .id_salt("request_editor-sa-1")
        .max_height(ui.available_height() - 40.0)
        .show(ui, |ui| {
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
        if ui
            .add_enabled(can_generate, egui::Button::new("Generate from response"))
            .clicked()
        {
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
        (
            "Status code",
            Check::StatusCode {
                op: NumberOp::Eq,
                value: 200,
            },
        ),
        ("Status class (2xx)", Check::StatusClass { class: 2 }),
        (
            "Header",
            Check::Header {
                name: String::new(),
                op: forge_core::model::StringOp::Exists,
                value: String::new(),
            },
        ),
        (
            "Content-Type",
            Check::ContentType {
                value: "application/json".to_string(),
            },
        ),
        (
            "JSON path",
            Check::JsonPath {
                path: "$.".to_string(),
                op: ValueOp::Exists,
                value: serde_json::Value::Null,
            },
        ),
        (
            "Body contains",
            Check::BodyContains {
                value: String::new(),
            },
        ),
        (
            "Body matches regex",
            Check::BodyMatches {
                regex: String::new(),
            },
        ),
        (
            "Response time below",
            Check::ResponseTimeBelow { max_ms: 1000 },
        ),
        (
            "JSON schema",
            Check::JsonSchema {
                schema: serde_json::json!({}),
            },
        ),
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
            SourceKind::JsonPath => ExtractorSource::JsonPath {
                expr: "$.".to_string(),
            },
            SourceKind::Header => ExtractorSource::Header {
                name: String::new(),
            },
            SourceKind::Regex => ExtractorSource::Regex {
                pattern: String::new(),
                group: 0,
            },
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
            egui::ComboBox::from_id_salt(("extract-kind", i))
                .selected_text(kind.label())
                .show_ui(ui, |ui| {
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
                .selected_text(if scope == ExtractScope::Runtime {
                    "Runtime"
                } else {
                    "Environment"
                })
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
            source: ExtractorSource::JsonPath {
                expr: "$.".to_string(),
            },
            var: String::new(),
            scope: ExtractScope::Runtime,
            enabled: true,
        });
        changed = true;
    }
    changed
}

/// A "Language: Rhai | JavaScript" combo box shared by the request editor's
/// Scripts tab and the collections tree's hooks editor dialog. Returns
/// `true` when the selection changed.
pub fn language_combo(ui: &mut Ui, id_salt: &str, lang: &mut ScriptLang) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Language:");
        let label = match lang {
            ScriptLang::Rhai => "Rhai",
            ScriptLang::Js => "JavaScript",
        };
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(lang, ScriptLang::Rhai, "Rhai")
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .selectable_value(lang, ScriptLang::Js, "JavaScript")
                    .changed()
                {
                    changed = true;
                }
            });
    });
    changed
}

fn scripts_tab(ui: &mut Ui, def: &mut RequestDef, scopes: &VarScopes) -> bool {
    let mut changed = false;
    if language_combo(ui, "req-scripts-lang", &mut def.scripts.language) {
        changed = true;
    }
    ui.add_space(4.0);
    ui.label("Pre-request script");
    let mut pre = def.scripts.pre_request.clone().unwrap_or_default();
    if code_editor(
        ui,
        "script-pre",
        &mut pre,
        Lang::Plain,
        Some(scopes),
        false,
        6,
        true,
    )
    .changed()
    {
        def.scripts.pre_request = if pre.is_empty() { None } else { Some(pre) };
        changed = true;
    }
    ui.add_space(6.0);
    ui.label("Post-response script");
    let mut post = def.scripts.post_response.clone().unwrap_or_default();
    if code_editor(
        ui,
        "script-post",
        &mut post,
        Lang::Plain,
        Some(scopes),
        false,
        6,
        true,
    )
    .changed()
    {
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
    checkbox_override(
        ui,
        "Timeout (ms)",
        &mut settings.timeout_ms,
        30_000,
        &mut changed,
        |ui, v, changed| {
            if ui.add(egui::DragValue::new(v).range(1..=600_000)).changed() {
                *changed = true;
            }
        },
    );
    checkbox_override(
        ui,
        "Follow redirects",
        &mut settings.follow_redirects,
        true,
        &mut changed,
        |ui, v, changed| {
            if ui.checkbox(v, "").changed() {
                *changed = true;
            }
        },
    );
    checkbox_override(
        ui,
        "Max redirects",
        &mut settings.max_redirects,
        10,
        &mut changed,
        |ui, v, changed| {
            if ui.add(egui::DragValue::new(v).range(0..=50)).changed() {
                *changed = true;
            }
        },
    );
    checkbox_override(
        ui,
        "Verify TLS certificates",
        &mut settings.verify_tls,
        true,
        &mut changed,
        |ui, v, changed| {
            if ui.checkbox(v, "").changed() {
                *changed = true;
            }
        },
    );
    if ui
        .checkbox(&mut settings.skip_in_runs, "Skip in collection runs")
        .changed()
    {
        changed = true;
    }
    changed
}
