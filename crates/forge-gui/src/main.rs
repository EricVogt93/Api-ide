#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod advisor;
mod app;
mod bridge;
mod git;
mod keymap;
mod local;
mod panels;
mod state;
mod theme;
mod updater;
mod widgets;

mod dialogs;

use std::path::PathBuf;

use app::{ForgeApp, DARK_WINDOW_ICON_PNG};

/// Forward `log` records (egui warns about widget-Id clashes there) to
/// stderr. Debug builds only — release stays silent.
#[cfg(debug_assertions)]
struct StderrLogger;

#[cfg(debug_assertions)]
impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }
    fn flush(&self) {}
}

fn main() -> eframe::Result {
    #[cfg(debug_assertions)]
    {
        static LOGGER: StderrLogger = StderrLogger;
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Warn));
    }

    // Optional CLI arg: a workspace directory to open on startup.
    let initial_workspace: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id("forge-ide")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([1100.0, 680.0])
        .with_title("Forge — API IDE");
    // Window/taskbar icon. A bad decode just means no icon, never a crash.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(DARK_WINDOW_ICON_PNG) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "forge-ide",
        options,
        Box::new(move |cc| {
            theme::fonts::install_fonts(&cc.egui_ctx);
            theme::ThemeKind::default().apply(&cc.egui_ctx);
            Ok(Box::new(ForgeApp::new(
                cc.egui_ctx.clone(),
                initial_workspace,
            )))
        }),
    )
}
