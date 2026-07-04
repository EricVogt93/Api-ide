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

    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono-Regular".to_owned());

    // Make the monospace font available as a fallback for the proportional
    // family too, so stray glyphs in labels don't fall back to tofu boxes.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("JetBrainsMono-Regular".to_owned());

    fonts
        .families
        .entry(FontFamily::Name("monospace-bold".into()))
        .or_default()
        .insert(0, "JetBrainsMono-Bold".to_owned());

    ctx.set_fonts(fonts);
}
