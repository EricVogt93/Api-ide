//! Forge Dark: neutral graphite surfaces, crisp hierarchy, one blue accent.

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

/// Background of the window chrome / outermost container.
#[allow(dead_code)]
pub const WINDOW_BG: Color32 = Color32::from_rgb(0x0D, 0x0F, 0x14);
/// Background of side/bottom panels, toolbars and the tab strip.
pub const PANEL_BG: Color32 = Color32::from_rgb(0x17, 0x1A, 0x21);
/// Background of the code/response editors.
pub const EDITOR_BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x17);
/// Default text color.
pub const TEXT: Color32 = Color32::from_rgb(0xE7, 0xEA, 0xF0);
/// Secondary/dimmed text (hints, timestamps, counters).
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8D, 0x96, 0xA8);
/// The New UI accent blue (focus rings, active tab underline, primary buttons).
pub const ACCENT: Color32 = Color32::from_rgb(0x5B, 0x7C, 0xFA);
/// List/text selection background.
pub const SELECTION: Color32 = Color32::from_rgb(0x26, 0x35, 0x66);
/// Hovered widget background.
pub const HOVERED: Color32 = Color32::from_rgb(0x22, 0x27, 0x31);
/// Active/pressed widget background.
pub const ACTIVE: Color32 = Color32::from_rgb(0x2A, 0x31, 0x40);
/// Border/outline color (subtle, New UI keeps borders faint).
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x30, 0x3B);
/// Seam between major regions (panels ↔ editor, strip separators) — the
/// near-black hairline JetBrains uses instead of a visible border.
pub const SEAM: Color32 = Color32::from_rgb(0x24, 0x2A, 0x34);
/// Hyperlink color.
pub const HYPERLINK: Color32 = Color32::from_rgb(0x79, 0x98, 0xFF);
/// Failure/error accent.
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x63, 0x72);
/// Warning accent.
pub const WARN: Color32 = Color32::from_rgb(0xF4, 0xB8, 0x60);
/// Success accent.
pub const OK: Color32 = Color32::from_rgb(0x47, 0xC9, 0x82);

const ROUNDING: u8 = 6;

/// Build the complete Dark [`egui::Style`].
pub fn style() -> Style {
    let mut style = Style::default();
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.window_fill = PANEL_BG;
    visuals.panel_fill = PANEL_BG;
    visuals.extreme_bg_color = EDITOR_BG;
    visuals.faint_bg_color = Color32::from_rgb(0x1C, 0x20, 0x28);
    visuals.code_bg_color = EDITOR_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = HYPERLINK;
    visuals.error_fg_color = ERROR;
    visuals.warn_fg_color = WARN;
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.popup_shadow.color = Color32::from_black_alpha(140);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    // Region separators (`ui.separator()`) and panel boundaries read as
    // near-black hairline seams, JetBrains-style, not visible gray borders.
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEAM);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x20, 0x24, 0x2C);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x20, 0x24, 0x2C);
    // New UI keeps resting widgets borderless — hierarchy comes from fills.
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.hovered.weak_bg_fill = HOVERED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x36, 0x3E, 0x4B));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.active.bg_fill = ACTIVE;
    visuals.widgets.active.weak_bg_fill = ACTIVE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.open.bg_fill = HOVERED;
    visuals.widgets.open.weak_bg_fill = HOVERED;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.open.corner_radius = CornerRadius::same(ROUNDING);

    visuals.selection.bg_fill = SELECTION;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    style.visuals = visuals;
    super::polish_spacing(&mut style);
    style
}
