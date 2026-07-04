//! A small colored pill showing an HTTP method, used in the collections
//! tree, tab bar and request editor.

use egui::{Color32, RichText, Ui};
use forge_core::model::Method;

/// Color conventionally associated with each HTTP method (loosely matching
/// Postman/Insomnia/IntelliJ HTTP client palettes).
pub fn method_color(method: Method) -> Color32 {
    match method {
        Method::Get => Color32::from_rgb(0x49, 0x9C, 0x54),
        Method::Post => Color32::from_rgb(0x35, 0x92, 0xC4),
        Method::Put => Color32::from_rgb(0xC7, 0x7D, 0x2E),
        Method::Patch => Color32::from_rgb(0x2A, 0xA1, 0x98),
        Method::Delete => Color32::from_rgb(0xC7, 0x54, 0x50),
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
            .size(11.0),
    );
}
