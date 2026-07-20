//! Icon glyphs. These are private-use codepoints from the vendored icon font
//! (VS Code *codicons* + a few Material Design icons, subset from JetBrains
//! Mono Nerd Font — see `theme::fonts`). The icon font is a fallback on both
//! font families, so these render inline in any label or `painter().text`
//! without selecting a family explicitly. Monochrome and stroked, they match
//! the JetBrains/Relay look far better than the emoji they replaced.

/// Left tool-window stripe: collections tree (a stack of layers).
pub const COLLECTIONS: &str = "\u{EBD2}"; // cod-layers
/// Environment variables (a globe) — reserved (env now lives in the top-bar pill).
#[allow(dead_code)]
pub const ENVIRONMENT: &str = "\u{EB01}"; // cod-globe
/// Left tool-window stripe: reqv1 asset store.
pub const ASSETS: &str = "\u{EACE}"; // cod-database
/// Bottom tool-window stripe: run/test results.
pub const RUN: &str = "\u{EB2C}"; // cod-play
/// Bottom tool-window stripe: history.
pub const HISTORY: &str = "\u{EA82}"; // cod-history
/// Bottom tool-window stripe: console/log output.
pub const CONSOLE: &str = "\u{EB9B}"; // cod-debug-console
/// Bottom tool-window stripe: cookies.
pub const COOKIES: &str = "\u{F0198}"; // md-cookie

/// Send/execute action (paper plane).
pub const PLAY: &str = "\u{EC0F}"; // cod-send
/// Stop/cancel action.
pub const STOP: &str = "\u{25A0}"; // ■ (no codicon in the subset)
/// Settings/gear.
pub const GEAR: &str = "\u{EAF8}"; // cod-gear
/// Theme toggle (half sun / half moon).
pub const THEME: &str = "\u{F050E}"; // md-theme-light-dark
/// Git branch (status bar).
pub const BRANCH: &str = "\u{F062C}"; // md-source-branch
/// Split layout: request/response stacked (horizontal divider).
pub const SPLIT_STACKED: &str = "\u{EB56}"; // cod-split-horizontal
/// Split layout: request/response side by side (vertical divider).
pub const SPLIT_SIDE: &str = "\u{EB57}"; // cod-split-vertical
/// Search (side-panel filter box).
pub const SEARCH: &str = "\u{EA6D}"; // cod-search
/// Save action.
pub const SAVE: &str = "\u{EB4B}"; // cod-save
/// Code / snippet export (`< >`).
pub const CODE: &str = "\u{EAC4}"; // cod-code
/// Copy to clipboard.
pub const COPY: &str = "\u{EBCC}"; // cod-copy
/// Add / new (`+`).
pub const ADD: &str = "\u{EA60}"; // cod-add
/// Overflow menu (`⋯`).
pub const ELLIPSIS: &str = "\u{EA7C}"; // cod-ellipsis
/// Activity / pulse (reserved rail icon).
#[allow(dead_code)]
pub const PULSE: &str = "\u{EB31}"; // cod-pulse
/// A single request leaf in the collections tree — reserved for a flatter
/// tree style without method badges.
#[allow(dead_code)]
pub const REQUEST: &str = "\u{2022}"; // •
/// Folder (closed) — kept for a future flat folder-icon tree style.
#[allow(dead_code)]
pub const FOLDER: &str = "\u{EA83}"; // cod-folder
/// Collapsed tree-row expand chevron.
pub const TRIANGLE_RIGHT: &str = "\u{EAB6}"; // cod-chevron-right
/// Expanded tree-row chevron.
pub const TRIANGLE_DOWN: &str = "\u{EAB4}"; // cod-chevron-down
/// Close ("x") glyph for tab close buttons.
pub const CLOSE: &str = "\u{EA76}"; // cod-close
/// Dirty-tab marker (small filled dot).
pub const DIRTY: &str = "\u{EB8A}"; // cod-circle-small-filled
/// Bottom tool-window stripe: problems (errors & warnings).
pub const PROBLEMS: &str = "\u{EA6C}"; // cod-warning
/// Bottom tool-window stripe: embedded terminal.
pub const TERMINAL: &str = "\u{EA85}"; // cod-terminal
/// Bottom tool-window stripe: application event log.
pub const LOG: &str = "\u{EB9D}"; // cod-output
/// Error severity marker.
pub const ERROR: &str = "\u{2716}"; // ✖
/// Warning severity marker.
pub const WARNING: &str = "\u{26A0}"; // ⚠
/// Tool-window collapse/hide affordance.
pub const COLLAPSE: &str = "\u{EABA}"; // cod-chrome-minimize
