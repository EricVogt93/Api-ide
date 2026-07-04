//! "Export code..." (request tab toolbar + collections context menu): render
//! a request as curl or a runnable snippet in various languages, with a
//! one-click copy to the clipboard.

use egui::{TextEdit, Window};

use forge_core::convert::{generate, to_curl, CurlExportOptions, SnippetLang};
use forge_core::model::RequestDef;

use crate::state::{AppState, StatusMessage};

/// Export target, layered over [`SnippetLang`] with the two curl flavors the
/// crate's `to_curl` renders via [`CurlExportOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    CurlMultiline,
    CurlOneLine,
    Snippet(SnippetLang),
}

impl Target {
    fn all() -> Vec<Target> {
        let mut v = vec![Target::CurlMultiline, Target::CurlOneLine];
        v.extend(SnippetLang::all().into_iter().map(Target::Snippet));
        v
    }

    fn label(&self) -> String {
        match self {
            Target::CurlMultiline => "curl (multiline)".to_string(),
            Target::CurlOneLine => "curl (one line)".to_string(),
            Target::Snippet(lang) => lang.label().to_string(),
        }
    }

    fn render(&self, def: &RequestDef) -> String {
        match self {
            Target::CurlMultiline => to_curl(def, &CurlExportOptions { multiline: true, long_flags: false }),
            Target::CurlOneLine => to_curl(def, &CurlExportOptions { multiline: false, long_flags: false }),
            Target::Snippet(lang) => generate(def, *lang),
        }
    }
}

/// Transient state of the code-export dialog, owned by
/// [`crate::dialogs::DialogManager`].
pub struct SnippetExportState {
    open: bool,
    target: Target,
    /// The request being exported (a standalone snapshot — the dialog
    /// doesn't need a live tab reference).
    def: Option<RequestDef>,
}

impl Default for SnippetExportState {
    fn default() -> Self {
        Self { open: false, target: Target::CurlMultiline, def: None }
    }
}

impl SnippetExportState {
    /// Open the dialog for a snapshot of `def`.
    pub fn open(&mut self, def: RequestDef) {
        self.open = true;
        self.def = Some(def);
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.snippet_export.open {
        return;
    }
    let Some(def) = state.dialogs.snippet_export.def.clone() else {
        state.dialogs.snippet_export.open = false;
        return;
    };

    let mut window_open = true;
    let mut copy_clicked = false;
    let mut preview = String::new();

    Window::new("Export code")
        .id(egui::Id::new("snippet-export-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 420.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Target:");
                egui::ComboBox::from_id_salt("snippet-export-target")
                    .selected_text(state.dialogs.snippet_export.target.label())
                    .show_ui(ui, |ui| {
                        for target in Target::all() {
                            ui.selectable_value(&mut state.dialogs.snippet_export.target, target, target.label());
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Copy to clipboard").clicked() {
                        copy_clicked = true;
                    }
                });
            });
            ui.add_space(6.0);
            ui.separator();

            preview = state.dialogs.snippet_export.target.render(&def);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut text = preview.clone();
                ui.add(
                    TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontSelection::from(egui::FontId::monospace(12.0)))
                        .interactive(false),
                );
            });
        });

    if copy_clicked {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(preview)) {
            Ok(()) => state.status = Some(StatusMessage::info("Copied to clipboard")),
            Err(e) => state.status = Some(StatusMessage::error(format!("clipboard error: {e}"))),
        }
    }
    if !window_open {
        state.dialogs.snippet_export.open = false;
    }
}
