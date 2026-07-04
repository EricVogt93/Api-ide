//! Small reusable UI building blocks shared by the panels.

pub mod code_editor;
pub mod kv_table;
pub mod method_badge;
pub mod response_view;

use egui::{Color32, Stroke, Ui};

/// A flat, IntelliJ-style underlined tab strip. Draws `tabs` as a row of
/// selectable labels, underlining the active one; returns `true` if the
/// selection changed this frame.
pub fn underline_tabs<T: Copy + PartialEq>(ui: &mut Ui, tabs: &[(T, &str)], selected: &mut T) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for (value, label) in tabs {
            let is_selected = *value == *selected;
            let color = if is_selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().text_color()
            };
            let text = if is_selected {
                egui::RichText::new(*label).color(color).strong()
            } else {
                egui::RichText::new(*label).color(color)
            };
            let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
            if is_selected {
                let rect = response.rect;
                let y = rect.bottom() + 2.0;
                ui.painter().line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    Stroke::new(2.0, color),
                );
            }
            if response.clicked() && !is_selected {
                *selected = *value;
                changed = true;
            }
        }
    });
    ui.add_space(2.0);
    ui.separator();
    changed
}

/// A small colored dot, used as a status/severity indicator. Reserved for a
/// future cookie/history tool window row status marker.
#[allow(dead_code)]
pub fn dot(ui: &mut Ui, color: Color32, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(radius * 2.0, radius * 2.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}
