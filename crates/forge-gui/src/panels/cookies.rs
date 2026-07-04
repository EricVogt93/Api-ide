//! Cookie manager tool window: lists cookies held by the bridge thread's
//! `HttpEngine` cookie jar and lets the user delete individual cookies or
//! clear the jar. The jar itself lives on the bridge thread (it's shared
//! with every HTTP request), so this panel only renders a cached snapshot
//! (`AppState::cookies_ui.rows`) refreshed via `Cmd::ListCookies` /
//! `Evt::Cookies`.

use std::path::{Path, PathBuf};

use egui_extras::{Column, TableBuilder};
use forge_core::exec::StoredCookie;

use crate::bridge::{Bridge, Cmd};
use crate::state::AppState;

/// Where a workspace's cookie jar is persisted, under `.forge-local/`.
pub fn cookies_path(root: &Path) -> PathBuf {
    root.join(forge_core::store::LOCAL_DIR).join("cookies.json")
}

/// Transient UI state for the cookie manager, held on [`AppState`].
#[derive(Default)]
pub struct CookiesUiState {
    pub rows: Vec<StoredCookie>,
    /// Set once the first `Cmd::ListCookies` has been issued for the
    /// current workspace, so `show` doesn't re-request it every frame.
    pub requested: bool,
}

/// Render the Cookies tool window.
pub fn show(ui: &mut egui::Ui, state: &mut AppState, bridge: &Bridge) {
    if !state.cookies_ui.requested {
        bridge.send(Cmd::ListCookies);
        state.cookies_ui.requested = true;
    }

    ui.horizontal(|ui| {
        if ui.button("Refresh").clicked() {
            bridge.send(Cmd::ListCookies);
        }
        if ui.button("Clear all").clicked() {
            bridge.send(Cmd::ClearCookies);
        }
    });
    ui.separator();

    let mut remove: Option<(String, String)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        TableBuilder::new(ui)
            .id_salt("cookies-table")
            .striped(true)
            .column(Column::auto().at_least(140.0).resizable(true))
            .column(Column::auto().at_least(70.0).resizable(true))
            .column(Column::auto().at_least(100.0).resizable(true))
            .column(Column::remainder().at_least(120.0))
            .column(Column::auto().at_least(150.0))
            .column(Column::auto().at_least(50.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::exact(28.0))
            .header(20.0, |mut header| {
                for label in ["Domain", "Path", "Name", "Value", "Expires", "Secure", "HttpOnly", ""] {
                    header.col(|ui| {
                        ui.strong(label);
                    });
                }
            })
            .body(|mut body| {
                for c in &state.cookies_ui.rows {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            ui.monospace(&c.domain);
                        });
                        row.col(|ui| {
                            ui.monospace(&c.path);
                        });
                        row.col(|ui| {
                            ui.monospace(&c.name);
                        });
                        row.col(|ui| {
                            ui.monospace(truncate(&c.value, 40));
                        });
                        row.col(|ui| {
                            ui.label(c.expires.map(|e| e.to_rfc3339()).unwrap_or_else(|| "session".to_string()));
                        });
                        row.col(|ui| {
                            ui.label(if c.secure { "yes" } else { "no" });
                        });
                        row.col(|ui| {
                            ui.label(if c.http_only { "yes" } else { "no" });
                        });
                        row.col(|ui| {
                            if ui.small_button("\u{2715}").clicked() {
                                remove = Some((c.domain.clone(), c.name.clone()));
                            }
                        });
                    });
                }
            });
    });

    if let Some((domain, name)) = remove {
        bridge.send(Cmd::RemoveCookie { domain, name });
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max_chars).collect::<String>())
    }
}
