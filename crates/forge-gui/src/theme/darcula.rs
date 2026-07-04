//! The "Darcula" dark theme — modelled on IntelliJ IDEA's default dark
//! look, built as a complete [`egui::Style`].

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

/// Background of the window chrome / outermost container.
pub const WINDOW_BG: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
/// Background of side/bottom panels and the tab strip.
pub const PANEL_BG: Color32 = Color32::from_rgb(0x3C, 0x3F, 0x41);
/// Background of the code/response editors.
pub const EDITOR_BG: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x2B);
/// Default text color.
pub const TEXT: Color32 = Color32::from_rgb(0xA9, 0xB7, 0xC6);
/// Selection highlight.
pub const SELECTION: Color32 = Color32::from_rgb(0x4B, 0x6E, 0xAF);
/// Hovered widget background.
pub const HOVERED: Color32 = Color32::from_rgb(0x4E, 0x52, 0x54);
/// Active/pressed widget background.
pub const ACTIVE: Color32 = Color32::from_rgb(0x5C, 0x61, 0x64);
/// Border/outline color.
pub const BORDER: Color32 = Color32::from_rgb(0x32, 0x32, 0x32);
/// Hyperlink color.
pub const HYPERLINK: Color32 = Color32::from_rgb(0x58, 0x9D, 0xF6);
/// Failure/error accent.
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x68);
/// Success accent.
pub const OK: Color32 = Color32::from_rgb(0x49, 0x9C, 0x54);

const ROUNDING: u8 = 3;

/// Build the complete Darcula [`egui::Style`].
pub fn style() -> Style {
    let mut style = Style::default();
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.window_fill = WINDOW_BG;
    visuals.panel_fill = PANEL_BG;
    visuals.extreme_bg_color = EDITOR_BG;
    visuals.faint_bg_color = Color32::from_rgb(0x32, 0x34, 0x36);
    visuals.code_bg_color = EDITOR_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = HYPERLINK;
    visuals.error_fg_color = ERROR;
    visuals.warn_fg_color = Color32::from_rgb(0xD8, 0xA6, 0x57);
    visuals.window_corner_radius = CornerRadius::same(ROUNDING);
    visuals.menu_corner_radius = CornerRadius::same(ROUNDING);
    visuals.window_stroke = Stroke::new(1.0, BORDER);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.inactive.bg_fill = PANEL_BG;
    visuals.widgets.inactive.weak_bg_fill = PANEL_BG;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.hovered.weak_bg_fill = HOVERED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, SELECTION);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.weak_bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, SELECTION);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.open.bg_fill = HOVERED;
    visuals.widgets.open.weak_bg_fill = HOVERED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, SELECTION);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = CornerRadius::same(ROUNDING);

    visuals.selection.bg_fill = SELECTION;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    style.visuals = visuals;
    style
}
