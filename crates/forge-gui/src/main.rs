#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bridge;
mod keymap;
mod local;
mod panels;
mod state;
mod theme;
mod widgets;

mod dialogs;

use std::path::PathBuf;

use app::ForgeApp;

fn main() -> eframe::Result {
    // Optional CLI arg: a workspace directory to open on startup.
    let initial_workspace: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Forge — API Test IDE"),
        ..Default::default()
    };

    eframe::run_native(
        "forge-ide",
        options,
        Box::new(move |cc| {
            theme::fonts::install_fonts(&cc.egui_ctx);
            theme::ThemeKind::default().apply(&cc.egui_ctx);
            Ok(Box::new(ForgeApp::new(cc.egui_ctx.clone(), initial_workspace)))
        }),
    )
}
