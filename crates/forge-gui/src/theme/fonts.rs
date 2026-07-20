//! Font setup: JetBrains Mono as the monospace family (used by the code
//! editor, response viewer and anywhere request/response data is shown),
//! with the default proportional family kept as a fallback.
//!
//! The TTFs are vendored under `assets/fonts/` (JetBrains Mono, OFL-1.1
//! licensed — see `assets/fonts/OFL.txt`) and baked into the binary via
//! `include_bytes!` so the app has no runtime font dependency.

use egui::{FontData, FontDefinitions, FontFamily};

const JETBRAINS_MONO_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
const JETBRAINS_MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");
/// Monochrome line-icon glyphs (VS Code *codicon* + a few Material Design
/// icons), subset from JetBrains Mono Nerd Font — private-use codepoints,
/// used across the shell instead of emoji so icons match the JetBrains/Relay
/// stroked look. See `theme::icons`.
const CODICONS: &[u8] = include_bytes!("../../assets/fonts/codicons.ttf");
/// IBM Plex Sans (OFL-1.1) — the Relay design's UI font. Latin subset,
/// instanced from the Google Fonts variable font at weights 400/500/600.
const PLEX_REGULAR: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf");
const PLEX_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Medium.ttf");
const PLEX_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf");

/// Install JetBrains Mono as the primary monospace font, keeping egui's
/// built-in proportional font as-is (with the mono family appended as a
/// fallback for glyphs it doesn't cover, and vice versa).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "JetBrainsMono-Regular".to_owned(),
        std::sync::Arc::new(FontData::from_static(JETBRAINS_MONO_REGULAR)),
    );
    fonts.font_data.insert(
        "JetBrainsMono-Bold".to_owned(),
        std::sync::Arc::new(FontData::from_static(JETBRAINS_MONO_BOLD)),
    );
    fonts.font_data.insert(
        "codicons".to_owned(),
        std::sync::Arc::new(FontData::from_static(CODICONS)),
    );
    fonts.font_data.insert(
        "IBMPlexSans-Regular".to_owned(),
        std::sync::Arc::new(FontData::from_static(PLEX_REGULAR)),
    );
    fonts.font_data.insert(
        "IBMPlexSans-Medium".to_owned(),
        std::sync::Arc::new(FontData::from_static(PLEX_MEDIUM)),
    );
    fonts.font_data.insert(
        "IBMPlexSans-SemiBold".to_owned(),
        std::sync::Arc::new(FontData::from_static(PLEX_SEMIBOLD)),
    );

    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono-Regular".to_owned());

    // IBM Plex Sans is the UI (proportional) font — the Relay design's
    // typeface. egui's built-in font stays as a glyph fallback, and the
    // monospace font after it so stray glyphs never render as tofu.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "IBMPlexSans-Regular".to_owned());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("JetBrainsMono-Regular".to_owned());

    // Named weight families for headings/wordmark (egui's `.strong()` only
    // recolors; real weight changes need a separate family).
    for (family, font) in [
        ("sans-medium", "IBMPlexSans-Medium"),
        ("sans-semibold", "IBMPlexSans-SemiBold"),
    ] {
        let list = fonts
            .families
            .entry(FontFamily::Name(family.into()))
            .or_default();
        list.insert(0, font.to_owned());
        list.push("codicons".to_owned());
    }

    // Append the icon font as a fallback on both families so the private-use
    // codepoints in `theme::icons` render in any label/painter text without
    // each call site having to select the family explicitly.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("codicons".to_owned());
    }

    fonts
        .families
        .entry(FontFamily::Name("monospace-bold".into()))
        .or_default()
        .insert(0, "JetBrainsMono-Bold".to_owned());

    ctx.set_fonts(fonts);
}
