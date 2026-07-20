//! Theming: complete [`egui::Style`]s for the two built-in themes (JetBrains
//! *New UI* Dark and Light), plus font setup and small icon glyph constants
//! used across the shell.

pub mod darcula;
pub mod fonts;
pub mod icons;
pub mod light;

/// The set of built-in themes the user can pick from the View menu / status
/// bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeKind {
    #[default]
    Darcula,
    Light,
}

/// Shared New-UI spacing polish applied by both theme builders: more air
/// between widgets, larger paddings, comfortable menus. Also bumps every
/// default text style +2px (13px body baseline reads too small on hidpi).
pub(crate) fn polish_spacing(style: &mut egui::Style) {
    for font in style.text_styles.values_mut() {
        font.size += 2.0;
    }
    // Tool windows snap open/closed instantly, IntelliJ-style — egui's
    // default size-lerp reads as sluggish since frames only repaint on input.
    style.animation_time = 0.0;
    // Debug builds: egui paints red outlines over every widget whose auto-id
    // shifted between the two layout passes of a frame (panel toggles resize
    // the editor, whose size-dependent branches then reorder auto-ids). The
    // frame freezes on screen until the next input, so users see a "red
    // skeleton". Purely cosmetic diagnostics — off.
    style.debug.warn_if_rect_changes_id = false;
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 6.0);
    s.button_padding = egui::vec2(10.0, 5.0);
    s.menu_margin = egui::Margin::same(6);
    s.window_margin = egui::Margin::same(10);
    s.icon_width = 16.0;
    s.icon_spacing = 6.0;
    s.interact_size.y = 24.0;
    s.combo_height = 240.0;
    s.scroll = egui::style::ScrollStyle::thin();
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 2] = [ThemeKind::Darcula, ThemeKind::Light];

    pub fn label(&self) -> &'static str {
        match self {
            ThemeKind::Darcula => "Dark",
            ThemeKind::Light => "Light",
        }
    }

    /// Build the complete [`egui::Style`] for this theme.
    pub fn style(&self) -> egui::Style {
        match self {
            ThemeKind::Darcula => darcula::style(),
            ThemeKind::Light => light::style(),
        }
    }

    /// Apply this theme to `ctx`.
    ///
    /// egui 0.35 keeps one [`egui::Style`] per built-in [`egui::Theme`] slot
    /// (Dark/Light) and switches between them via `Context::set_theme`; our
    /// two themes map onto those two slots one-to-one, so installing ours
    /// there and selecting the matching slot is enough to make it active
    /// everywhere (including in any egui-internal widgets that peek at
    /// `Theme::default_visuals`).
    pub fn apply(&self, ctx: &egui::Context) {
        let theme = match self {
            ThemeKind::Darcula => egui::Theme::Dark,
            ThemeKind::Light => egui::Theme::Light,
        };
        ctx.set_style_of(theme, self.style());
        ctx.set_theme(theme);
    }

    /// Accent color for "ok"/success (2xx, passing assertion, …).
    pub fn ok_color(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::OK,
            ThemeKind::Light => light::OK,
        }
    }

    /// Accent color for errors / failures.
    pub fn error_color(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::ERROR,
            ThemeKind::Light => light::ERROR,
        }
    }

    /// Accent color for warnings.
    pub fn warn_color(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::WARN,
            ThemeKind::Light => light::WARN,
        }
    }

    /// The New UI accent blue (focus, active-tab underline, primary action).
    pub fn accent_color(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::ACCENT,
            ThemeKind::Light => light::ACCENT,
        }
    }

    /// Dimmed/secondary text color (hints, timestamps, counters).
    pub fn dim_color(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::TEXT_DIM,
            ThemeKind::Light => light::TEXT_DIM,
        }
    }

    /// Background color used by the read-only code/response editors.
    pub fn editor_bg(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::EDITOR_BG,
            ThemeKind::Light => light::EDITOR_BG,
        }
    }
}
