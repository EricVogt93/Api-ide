//! The "Light" theme — JetBrains *New UI* light look: white editor, very
//! light gray `#F7F8FA` islands, faint `#EBECF0` borders and the shared
//! `#3574F0` accent.

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

/// Background of the window chrome / outermost container.
#[allow(dead_code)]
pub const WINDOW_BG: Color32 = Color32::from_rgb(0xF7, 0xF8, 0xFA);
/// Background of side/bottom panels, toolbars and the tab strip.
pub const PANEL_BG: Color32 = Color32::from_rgb(0xF7, 0xF8, 0xFA);
/// Background of the code/response editors.
pub const EDITOR_BG: Color32 = Color32::WHITE;
/// Default text color.
pub const TEXT: Color32 = Color32::from_rgb(0x27, 0x28, 0x2E);
/// Secondary/dimmed text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x6C, 0x70, 0x7E);
/// The New UI accent blue.
pub const ACCENT: Color32 = Color32::from_rgb(0x35, 0x74, 0xF0);
/// List/text selection background.
pub const SELECTION: Color32 = Color32::from_rgb(0xD4, 0xE2, 0xFF);
/// Hovered widget background.
pub const HOVERED: Color32 = Color32::from_rgb(0xE8, 0xEA, 0xEC);
/// Active/pressed widget background.
pub const ACTIVE: Color32 = Color32::from_rgb(0xDF, 0xE1, 0xE5);
/// Border/outline color.
pub const BORDER: Color32 = Color32::from_rgb(0xEB, 0xEC, 0xF0);
/// Hyperlink color.
pub const HYPERLINK: Color32 = Color32::from_rgb(0x2E, 0x55, 0xA3);
/// Failure/error accent.
pub const ERROR: Color32 = Color32::from_rgb(0xDB, 0x3B, 0x4B);
/// Warning accent.
pub const WARN: Color32 = Color32::from_rgb(0xB0, 0x7D, 0x02);
/// Success accent.
pub const OK: Color32 = Color32::from_rgb(0x20, 0x8A, 0x3C);

const ROUNDING: u8 = 6;

/// Build the complete Light [`egui::Style`].
pub fn style() -> Style {
    let mut style = Style::default();
    let mut visuals = Visuals::light();

    visuals.dark_mode = false;
    visuals.window_fill = PANEL_BG;
    visuals.panel_fill = PANEL_BG;
    visuals.extreme_bg_color = EDITOR_BG;
    visuals.faint_bg_color = Color32::from_rgb(0xEF, 0xF1, 0xF3);
    visuals.code_bg_color = EDITOR_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = HYPERLINK;
    visuals.error_fg_color = ERROR;
    visuals.warn_fg_color = WARN;
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(0xD3, 0xD5, 0xDB));
    visuals.popup_shadow.color = Color32::from_black_alpha(40);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0xED, 0xEE, 0xF0);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0xED, 0xEE, 0xF0);
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.hovered.weak_bg_fill = HOVERED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xC9, 0xCC, 0xD1));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.weak_bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::BLACK);
    visuals.widgets.active.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.open.bg_fill = HOVERED;
    visuals.widgets.open.weak_bg_fill = HOVERED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = CornerRadius::same(ROUNDING);

    visuals.selection.bg_fill = SELECTION;
    visuals.selection.stroke = Stroke::NONE;

    style.visuals = visuals;
    super::polish_spacing(&mut style);
    style
}
