#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
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
        Box::new(|_cc| Ok(Box::new(Placeholder))),
    )
}

struct Placeholder;

impl eframe::App for Placeholder {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| ui.heading("Forge"));
        });
    }
}
