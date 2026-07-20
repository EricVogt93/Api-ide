//! The "Dark" theme — modelled on JetBrains' modern *New UI* dark look
//! (IntelliJ 2022.3+): near-black `#1E1F22` surfaces, `#2B2D30` islands,
//! the signature `#3574F0` accent blue and generous, flat spacing.

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

/// Background of the window chrome / outermost container.
#[allow(dead_code)]
pub const WINDOW_BG: Color32 = Color32::from_rgb(0x1E, 0x1F, 0x22);
/// Background of side/bottom panels, toolbars and the tab strip.
pub const PANEL_BG: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x30);
/// Background of the code/response editors.
pub const EDITOR_BG: Color32 = Color32::from_rgb(0x1E, 0x1F, 0x22);
/// Default text color.
pub const TEXT: Color32 = Color32::from_rgb(0xDF, 0xE1, 0xE5);
/// Secondary/dimmed text (hints, timestamps, counters).
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9D, 0xA0, 0xA8);
/// The New UI accent blue (focus rings, active tab underline, primary buttons).
pub const ACCENT: Color32 = Color32::from_rgb(0x35, 0x74, 0xF0);
/// List/text selection background.
pub const SELECTION: Color32 = Color32::from_rgb(0x2E, 0x43, 0x6E);
/// Hovered widget background.
pub const HOVERED: Color32 = Color32::from_rgb(0x39, 0x3B, 0x40);
/// Active/pressed widget background.
pub const ACTIVE: Color32 = Color32::from_rgb(0x43, 0x45, 0x4A);
/// Border/outline color (subtle, New UI keeps borders faint).
pub const BORDER: Color32 = Color32::from_rgb(0x39, 0x3B, 0x40);
/// Seam between major regions (panels ↔ editor, strip separators) — the
/// near-black hairline JetBrains uses instead of a visible border.
pub const SEAM: Color32 = Color32::from_rgb(0x13, 0x14, 0x17);
/// Hyperlink color.
pub const HYPERLINK: Color32 = Color32::from_rgb(0x54, 0x8A, 0xF7);
/// Failure/error accent.
pub const ERROR: Color32 = Color32::from_rgb(0xF7, 0x54, 0x64);
/// Warning accent.
pub const WARN: Color32 = Color32::from_rgb(0xF2, 0xC5, 0x5C);
/// Success accent.
pub const OK: Color32 = Color32::from_rgb(0x5F, 0xAD, 0x65);

const ROUNDING: u8 = 6;

/// Build the complete Dark [`egui::Style`].
pub fn style() -> Style {
    let mut style = Style::default();
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.window_fill = PANEL_BG;
    visuals.panel_fill = PANEL_BG;
    visuals.extreme_bg_color = EDITOR_BG;
    visuals.faint_bg_color = Color32::from_rgb(0x26, 0x28, 0x2C);
    visuals.code_bg_color = EDITOR_BG;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = HYPERLINK;
    visuals.error_fg_color = ERROR;
    visuals.warn_fg_color = WARN;
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.popup_shadow.color = Color32::from_black_alpha(96);

    visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL_BG;
    // Region separators (`ui.separator()`) and panel boundaries read as
    // near-black hairline seams, JetBrains-style, not visible gray borders.
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SEAM);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x35, 0x37, 0x3B);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x35, 0x37, 0x3B);
    // New UI keeps resting widgets borderless — hierarchy comes from fills.
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(ROUNDING);

    visuals.widgets.hovered.bg_fill = HOVERED;
    visuals.widgets.hovered.weak_bg_fill = HOVERED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4A, 0x4D, 0x53));
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
    visuals.selection.stroke = Stroke::NONE;

    style.visuals = visuals;
    super::polish_spacing(&mut style);
    style
}
