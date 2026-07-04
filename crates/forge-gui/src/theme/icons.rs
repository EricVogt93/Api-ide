//! Minimal icon helpers. egui ships no icon font, so tool-window and
//! toolbar glyphs are plain Unicode symbols chosen to render reliably with
//! the default proportional font — good enough for a shell; a real icon set
//! can replace these one call-site at a time later.

/// Left tool-window stripe: collections tree.
pub const COLLECTIONS: &str = "\u{1F4C1}"; // 📁
/// Right tool-window stripe: environment variables.
pub const ENVIRONMENT: &str = "\u{1F30D}"; // 🌍
/// Bottom tool-window stripe: run/test results.
pub const RUN: &str = "\u{25B6}"; // ▶
/// Bottom tool-window stripe: history.
pub const HISTORY: &str = "\u{1F551}"; // 🕑
/// Bottom tool-window stripe: console/log output.
pub const CONSOLE: &str = "\u{2328}"; // ⌨
/// Bottom tool-window stripe: cookies.
pub const COOKIES: &str = "\u{1F36A}"; // 🍪

/// Send/execute action.
pub const PLAY: &str = "\u{25B6}"; // ▶
/// Stop/cancel action.
pub const STOP: &str = "\u{25A0}"; // ■
/// Settings/gear — reserved for a future per-row run-configuration menu.
#[allow(dead_code)]
pub const GEAR: &str = "\u{2699}"; // ⚙
/// A single request leaf in the collections tree — reserved for a flatter
/// tree style without method badges.
#[allow(dead_code)]
pub const REQUEST: &str = "\u{2022}"; // •
/// Folder (closed).
pub const FOLDER: &str = "\u{1F4C2}"; // 📂
/// Collapsed tree-row expand triangle.
pub const TRIANGLE_RIGHT: &str = "\u{25B8}"; // ▸
/// Expanded tree-row triangle.
pub const TRIANGLE_DOWN: &str = "\u{25BE}"; // ▾
/// Close ("x") glyph for tab close buttons.
pub const CLOSE: &str = "\u{2715}"; // ✕
/// Dirty-tab marker.
pub const DIRTY: &str = "\u{25CF}"; // ●
