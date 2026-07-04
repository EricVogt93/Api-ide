//! Renders the response side of a request tab: status/time/size header,
//! and Body/Raw/Headers/Timing/Assertions sub-tabs.

use egui::{Color32, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use forge_core::runner::RequestOutcome;

use crate::theme::ThemeKind;
use crate::widgets::code_editor::{code_editor, Lang};
use crate::widgets::underline_tabs;

/// Which response sub-tab is active; persisted per-tab in [`crate::state::Tab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseTab {
    #[default]
    Body,
    Raw,
    Headers,
    Timing,
    Assertions,
}

/// Per-tab UI state for the response viewer.
#[derive(Debug, Clone, Default)]
pub struct ResponseViewState {
    pub tab: ResponseTab,
    pub wrap: bool,
    /// Scratch text buffer for the (read-only) body/raw editors, rebuilt
    /// whenever the underlying response changes.
    pub body_text: String,
    pub raw_text: String,
}

impl ResponseViewState {
    /// Rebuild the cached body/raw text buffers from a freshly received
    /// response. Call this once when a tab's response outcome changes.
    pub fn sync(&mut self, outcome: Option<&RequestOutcome>) {
        self.body_text.clear();
        self.raw_text.clear();
        if let Some(Ok(exec)) = outcome.map(|o| &o.result) {
            self.raw_text = exec.text().into_owned();
            self.body_text = if exec.is_json() {
                exec.json()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_else(|| exec.text().into_owned())
            } else {
                exec.text().into_owned()
            };
        }
    }
}

/// Status pill color by response status class.
fn status_color(theme: ThemeKind, status: u16) -> Color32 {
    match status / 100 {
        2 => theme.ok_color(),
        3 => Color32::from_rgb(0x35, 0x92, 0xC4),
        4 => Color32::from_rgb(0xC7, 0x7D, 0x2E),
        5 => theme.error_color(),
        _ => Color32::GRAY,
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Render the response panel. `state` is the tab-local UI state (already
/// synced via [`ResponseViewState::sync`] when the response last changed).
pub fn response_view(ui: &mut Ui, outcome: Option<&RequestOutcome>, state: &mut ResponseViewState, theme: ThemeKind) {
    let Some(outcome) = outcome else {
        ui.centered_and_justified(|ui| {
            ui.weak("Send a request to see the response here.");
        });
        return;
    };

    let exec = match &outcome.result {
        Ok(exec) => exec,
        Err(err) => {
            egui::Frame::NONE
                .fill(theme.error_color().gamma_multiply(0.15))
                .inner_margin(8.0)
                .corner_radius(3u8)
                .show(ui, |ui| {
                    ui.colored_label(theme.error_color(), format!("Request failed: {err}"));
                });
            return;
        }
    };

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} {}", exec.status, exec.status_text))
                .color(status_color(theme, exec.status))
                .strong(),
        );
        ui.separator();
        ui.label(format!("{} ms", exec.timing.total.as_millis()));
        ui.separator();
        ui.label(human_size(exec.size.body_bytes));
        ui.separator();
        ui.weak(&exec.effective_url);
    });
    ui.add_space(4.0);

    let tabs: &[(ResponseTab, &str)] = &[
        (ResponseTab::Body, "Body"),
        (ResponseTab::Raw, "Raw"),
        (ResponseTab::Headers, "Headers"),
        (ResponseTab::Timing, "Timing"),
        (ResponseTab::Assertions, "Assertions"),
    ];
    underline_tabs(ui, tabs, &mut state.tab);

    match state.tab {
        ResponseTab::Body => {
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(state.body_text.clone()));
                }
                ui.checkbox(&mut state.wrap, "Wrap");
            });
            let lang = if exec.is_json() { Lang::Json } else { Lang::Plain };
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                code_editor(ui, "resp-body", &mut state.body_text, lang, None, true, 6, state.wrap);
            });
        }
        ResponseTab::Raw => {
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(state.raw_text.clone()));
                }
                ui.checkbox(&mut state.wrap, "Wrap");
            });
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                code_editor(ui, "resp-raw", &mut state.raw_text, Lang::Plain, None, true, 6, state.wrap);
            });
        }
        ResponseTab::Headers => {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("resp-headers")
                    .striped(true)
                    .column(Column::auto().at_least(120.0).resizable(true))
                    .column(Column::remainder().at_least(120.0))
                    .header(20.0, |mut header| {
                        header.col(|ui| {
                            ui.strong("Name");
                        });
                        header.col(|ui| {
                            ui.strong("Value");
                        });
                    })
                    .body(|mut body| {
                        for (k, v) in &exec.headers {
                            body.row(20.0, |mut row| {
                                row.col(|ui| {
                                    ui.monospace(k);
                                });
                                row.col(|ui| {
                                    ui.monospace(v);
                                });
                            });
                        }
                    });
            });
        }
        ResponseTab::Timing => {
            ui.add_space(4.0);
            let ttfb = exec.timing.ttfb.as_secs_f32();
            let download = exec.timing.download.as_secs_f32();
            let total = exec.timing.total.as_secs_f32().max(0.001);
            let width = ui.available_width().max(100.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
            let painter = ui.painter();
            let ttfb_w = width * (ttfb / total).clamp(0.0, 1.0);
            let download_w = width * (download / total).clamp(0.0, 1.0 - ttfb_w / width);
            let mut x = rect.left();
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(ttfb_w, rect.height())),
                0u8,
                Color32::from_rgb(0x35, 0x92, 0xC4),
            );
            x += ttfb_w;
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(download_w, rect.height())),
                0u8,
                theme.ok_color(),
            );
            ui.add_space(6.0);
            ui.label(format!("Time to first byte: {} ms", exec.timing.ttfb.as_millis()));
            ui.label(format!("Download: {} ms", exec.timing.download.as_millis()));
            ui.label(format!("Total: {} ms", exec.timing.total.as_millis()));
        }
        ResponseTab::Assertions => {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if outcome.assertions.is_empty() {
                    ui.weak("No assertions configured for this request.");
                }
                for assertion in &outcome.assertions {
                    ui.horizontal(|ui| {
                        if assertion.passed {
                            ui.colored_label(theme.ok_color(), "\u{2713}");
                        } else {
                            ui.colored_label(theme.error_color(), "\u{2715}");
                        }
                        ui.label(&assertion.summary);
                        if let Some(msg) = &assertion.message {
                            ui.weak(msg);
                        }
                    });
                }
            });
        }
    }
}
