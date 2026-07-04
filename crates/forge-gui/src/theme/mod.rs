//! Theming: complete [`egui::Style`]s for the two built-in themes, plus font
//! setup and small icon glyph constants used across the shell.

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

impl ThemeKind {
    pub const ALL: [ThemeKind; 2] = [ThemeKind::Darcula, ThemeKind::Light];

    pub fn label(&self) -> &'static str {
        match self {
            ThemeKind::Darcula => "Darcula",
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

    /// Background color used by the read-only code/response editors.
    pub fn editor_bg(&self) -> egui::Color32 {
        match self {
            ThemeKind::Darcula => darcula::EDITOR_BG,
            ThemeKind::Light => light::EDITOR_BG,
        }
    }
}
