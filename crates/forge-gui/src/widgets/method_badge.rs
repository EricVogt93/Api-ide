//! A small colored pill showing an HTTP method, used in the collections
//! tree, tab bar and request editor.

use egui::{Color32, RichText, Ui};
use forge_core::model::Method;

/// Color associated with each HTTP method — the Relay design palette
/// (GET green, POST amber, PUT blue, PATCH violet, DELETE red).
pub fn method_color(method: Method) -> Color32 {
    match method {
        Method::Get => Color32::from_rgb(0x59, 0xA8, 0x69),
        Method::Post => Color32::from_rgb(0xD9, 0xA3, 0x43),
        Method::Put => Color32::from_rgb(0x4A, 0x90, 0xD9),
        Method::Patch => Color32::from_rgb(0xC5, 0x86, 0xC0),
        Method::Delete => Color32::from_rgb(0xDB, 0x5C, 0x5C),
        Method::Head | Method::Options | Method::Trace => Color32::from_gray(0x9E),
    }
}

/// Draw a compact, right-aligned-width method badge (e.g. `GET`, `POST`).
pub fn method_badge(ui: &mut Ui, method: Method) {
    let color = method_color(method);
    ui.label(
        RichText::new(method.as_str())
            .color(color)
            .monospace()
            .strong()
            .size(13.0),
    );
}
