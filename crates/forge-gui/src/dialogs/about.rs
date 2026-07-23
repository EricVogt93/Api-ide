//! About ApiWright (Help menu): version/build info and third-party credits.

use egui::Window;

use crate::state::AppState;

/// Render the About window if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.about_open {
        return;
    }
    let mut open = state.dialogs.about_open;
    Window::new("About ApiWright")
        .id(egui::Id::new("about-dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.heading("ApiWright");
            ui.label("A local-first IDE for building, inspecting, and verifying APIs.");
            ui.add_space(8.0);
            egui::Grid::new("about-grid")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Version");
                    ui.monospace(env!("CARGO_PKG_VERSION"));
                    ui.end_row();

                    ui.label("Workspace format");
                    ui.monospace(forge_core::FORMAT_VERSION.to_string());
                    ui.end_row();

                    ui.label("egui");
                    ui.monospace("0.35");
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.weak("Licensed under the terms in LICENSE.");
            ui.weak(
                "Monospace UI font: JetBrains Mono, licensed under the SIL Open Font License 1.1.",
            );
        });
    state.dialogs.about_open = open;
}
