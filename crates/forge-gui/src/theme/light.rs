//! The "IntelliJ Light" theme, built as a complete [`egui::Style`].

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

/// Background of panels and the tab strip.
pub const PANEL_BG: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);
/// Background of the code/response editors.
pub const EDITOR_BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// Default text color.
pub const TEXT: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
/// Selection highlight.
pub const SELECTION: Color32 = Color32::from_rgb(0xA6, 0xD2, 0xFF);
/// Border/outline color.
pub const BORDER: Color32 = Color32::from_rgb(0xC9, 0xC9, 0xC9);
/// Hovered widget background.
pub const HOVERED: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
/// Active/pressed widget background.
pub const ACTIVE: Color32 = Color32::from_rgb(0xD8, 0xD8, 0xD8);
/// Hyperlink color.
pub const HYPERLINK: Color32 = Color32::from_rgb(0x28, 0x60, 0xC4);
/// Failure/error accent.
pub const ERROR: Color32 = Color32::from_rgb(0xC7, 0x54, 0x50);
/// Success accent.
pub const OK: Color32 = Color32::from_rgb(0x2D, 0x8B, 0x39);

const ROUNDING: u8 = 3;

/// Build the complete IntelliJ Light [`egui::Style`].
pub fn style() -> Style {
    let mut style = Style::default();
    let mut visuals = Visuals::light();

    visuals.dark_mode = false;
    visuals.window_fill = PANEL_BG;
    visuals.panel_fill = PANEL_BG;
    visuals.extreme_bg_color = EDITOR_BG;
    visuals.faint_bg_color = Color32::from_rgb(0xEA, 0xEA, 0xEA);
    visuals.code_bg_color = EDITOR_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = HYPERLINK;
    visuals.error_fg_color = ERROR;
    visuals.warn_fg_color = Color32::from_rgb(0x9A, 0x6A, 0x00);
    visuals.window_corner_radius = CornerRadius::same(ROUNDING);
    visuals.menu_corner_radius = CornerRadius::same(ROUNDING);
    visuals.window_stroke = Stroke::new(1.0, BORDER);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0xE4, 0xE4, 0xE4);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0xE4, 0xE4, 0xE4);
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
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.active.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.open.bg_fill = HOVERED;
    visuals.widgets.open.weak_bg_fill = HOVERED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, SELECTION);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = CornerRadius::same(ROUNDING);

    visuals.selection.bg_fill = SELECTION;
    visuals.selection.stroke = Stroke::new(1.0, TEXT);

    style.visuals = visuals;
    style
}
