//! A multiline monospace text editor with hand-rolled syntax highlighting
//! for JSON, XML and GraphQL, plus `{{variable}}` span highlighting shared
//! with the URL bar. Used for request/response bodies and pre/post scripts;
//! the read-only mode is reused by the response viewer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, FontSelection, TextEdit, Ui};

use forge_core::vars::{spans, VarScopes};

/// Editor language, controlling which lexer produces syntax colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Json,
    Xml,
    GraphQl,
    Plain,
}

struct Palette {
    string: Color32,
    number: Color32,
    keyword: Color32,
    key: Color32,
    comment: Color32,
    text: Color32,
    var_fg: Color32,
    var_bg: Color32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            string: Color32::from_rgb(0x6A, 0x87, 0x59),
            number: Color32::from_rgb(0x68, 0x97, 0xBB),
            keyword: Color32::from_rgb(0xCC, 0x78, 0x32),
            key: Color32::from_rgb(0x98, 0x76, 0xAA),
            comment: Color32::from_rgb(0x80, 0x80, 0x80),
            text: Color32::from_rgb(0xA9, 0xB7, 0xC6),
            var_fg: Color32::from_rgb(0xFF, 0xC6, 0x6D),
            var_bg: Color32::from_rgba_unmultiplied(0xFF, 0xC6, 0x6D, 30),
        }
    } else {
        Palette {
            string: Color32::from_rgb(0x00, 0x80, 0x00),
            number: Color32::from_rgb(0x1A, 0x1A, 0xA6),
            keyword: Color32::from_rgb(0x00, 0x00, 0xFF),
            key: Color32::from_rgb(0x66, 0x0E, 0x7A),
            comment: Color32::from_rgb(0x8C, 0x8C, 0x8C),
            text: Color32::from_rgb(0x00, 0x00, 0x00),
            var_fg: Color32::from_rgb(0xB3, 0x6B, 0x00),
            var_bg: Color32::from_rgba_unmultiplied(0xB3, 0x6B, 0x00, 30),
        }
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, LayoutJob>> = RefCell::new(HashMap::new());
}

fn cache_key(text: &str, dark: bool, lang: Lang) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    dark.hash(&mut h);
    lang.hash(&mut h);
    h.finish()
}

/// Show an editable (or read-only) multiline code editor.
///
/// `scopes` is used to resolve `{{variable}}` spans for highlighting only;
/// pass `None` to skip variable highlighting entirely (e.g. for response
/// bodies, which are never templated).
#[allow(clippy::too_many_arguments)]
pub fn code_editor(
    ui: &mut Ui,
    id_salt: &str,
    text: &mut String,
    lang: Lang,
    scopes: Option<&VarScopes>,
    read_only: bool,
    min_rows: usize,
    wrap: bool,
) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let var_spans = scopes.map(|s| spans(text, s)).unwrap_or_default();
    let desired_width = if wrap {
        ui.available_width()
    } else {
        f32::INFINITY
    };

    let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        let text = buf.as_str();
        let key = cache_key(text, dark, lang);
        let mut job = CACHE.with(|c| c.borrow().get(&key).cloned());
        if job.is_none() {
            let built = build_layout_job(text, lang, dark, &var_spans);
            CACHE.with(|c| {
                let mut map = c.borrow_mut();
                if map.len() > 200 {
                    map.clear();
                }
                map.insert(key, built.clone());
            });
            job = Some(built);
        }
        let mut job = job.unwrap_or_default();
        job.wrap.max_width = wrap_width;
        ui.fonts_mut(|f| f.layout_job(job))
    };

    ui.add(
        TextEdit::multiline(text)
            .id_salt(id_salt)
            .font(FontSelection::from(FontId::monospace(15.0)))
            .desired_width(desired_width)
            .desired_rows(min_rows)
            .code_editor()
            .interactive(!read_only)
            .layouter(&mut layouter),
    )
}

/// A left-hand line-number gutter, monospace and dimmed, JetBrains/Relay
/// style. Rendered as a column of right-aligned numbers whose row height
/// matches the code editor's, so placed beside [`code_editor`] inside the
/// same scroll viewport they scroll and align together (with wrapping off).
pub fn line_gutter(ui: &mut Ui, lines: usize) {
    let color = ui.visuals().weak_text_color();
    let font = FontId::monospace(15.0);
    let width = 3.max(lines.to_string().len()) as f32 * 8.0 + 12.0;
    ui.allocate_ui(egui::vec2(width, ui.available_height()), |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // Match TextEdit's top inner margin so row 1 lines up.
            ui.add_space(2.0);
            for i in 1..=lines.max(1) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(i.to_string())
                            .font(font.clone())
                            .color(color),
                    );
                });
            }
        });
    });
}

/// [`code_editor`] with a line-number gutter down the left edge. Intended to
/// be called inside a scroll area so the gutter and text scroll as one.
#[allow(clippy::too_many_arguments)]
pub fn code_editor_numbered(
    ui: &mut Ui,
    id_salt: &str,
    text: &mut String,
    lang: Lang,
    scopes: Option<&VarScopes>,
    read_only: bool,
    min_rows: usize,
    wrap: bool,
) -> egui::Response {
    let lines = text.lines().count().max(min_rows);
    ui.horizontal_top(|ui| {
        line_gutter(ui, lines);
        code_editor(ui, id_salt, text, lang, scopes, read_only, min_rows, wrap)
    })
    .inner
}

fn build_layout_job(
    text: &str,
    lang: Lang,
    dark: bool,
    var_spans: &[forge_core::vars::VarSpan],
) -> LayoutJob {
    let pal = palette(dark);
    let font = FontId::monospace(15.0);

    let base: Vec<(usize, usize, Color32)> = match lang {
        Lang::Json => lex_json(text, &pal),
        Lang::Xml => lex_xml(text, &pal),
        Lang::GraphQl => lex_graphql(text, &pal),
        Lang::Plain => Vec::new(),
    };

    // Boundary points from both base tokens and variable spans.
    let mut bounds: Vec<usize> = Vec::with_capacity(base.len() * 2 + var_spans.len() * 2 + 2);
    bounds.push(0);
    bounds.push(text.len());
    for (s, e, _) in &base {
        bounds.push(*s);
        bounds.push(*e);
    }
    for v in var_spans {
        bounds.push(v.start);
        bounds.push(v.end);
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut job = LayoutJob::default();
    for w in bounds.windows(2) {
        let (s, e) = (w[0], w[1]);
        if s >= e || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            continue;
        }
        let var_hit = var_spans.iter().any(|v| v.start <= s && e <= v.end);
        let color = if var_hit {
            pal.var_fg
        } else {
            base.iter()
                .filter(|(bs, be, _)| *bs <= s && e <= *be)
                .min_by_key(|(bs, be, _)| be - bs)
                .map(|(_, _, c)| *c)
                .unwrap_or(pal.text)
        };
        let mut fmt = TextFormat::simple(font.clone(), color);
        if var_hit {
            fmt.background = pal.var_bg;
        }
        job.append(&text[s..e], 0.0, fmt);
    }
    job
}

/// Minimal JSON lexer: strings (keys vs. values), numbers, `true`/`false`/`null`.
fn lex_json(text: &str, pal: &Palette) -> Vec<(usize, usize, Color32)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                let end = i.min(bytes.len());
                let mut j = end;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                let is_key = j < bytes.len() && bytes[j] == b':';
                out.push((start, end, if is_key { pal.key } else { pal.string }));
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push((start, i, pal.number));
            }
            b't' | b'f' | b'n' => {
                let mut matched = false;
                for kw in ["true", "false", "null"] {
                    if text[i..].starts_with(kw) {
                        out.push((i, i + kw.len(), pal.keyword));
                        i += kw.len();
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Cheap XML/HTML tag lexer: tags get the "key" color, quoted attribute
/// values get the "string" color, comments get the "comment" color.
fn lex_xml(text: &str, pal: &Palette) -> Vec<(usize, usize, Color32)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if text[i..].starts_with("<!--") {
                let end = text[i..]
                    .find("-->")
                    .map(|p| i + p + 3)
                    .unwrap_or(text.len());
                out.push((i, end, pal.comment));
                i = end;
                continue;
            }
            let end = text[i..].find('>').map(|p| i + p + 1).unwrap_or(text.len());
            out.push((i, end, pal.key));
            let tag = &text[i..end];
            let mut j = 0usize;
            while let Some(q) = tag[j..].find(['"', '\'']) {
                let qpos = j + q;
                let qchar = tag.as_bytes()[qpos] as char;
                if let Some(rel) = tag[qpos + 1..].find(qchar) {
                    let end2 = qpos + 1 + rel + 1;
                    out.push((i + qpos, i + end2, pal.string));
                    j = end2;
                } else {
                    break;
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

const GRAPHQL_KEYWORDS: &[&str] = &[
    "query",
    "mutation",
    "subscription",
    "fragment",
    "on",
    "true",
    "false",
    "null",
    "type",
    "input",
    "enum",
    "interface",
    "implements",
    "scalar",
    "union",
    "directive",
    "schema",
];

/// Minimal GraphQL lexer: keywords, `$variables`, strings and `#` comments.
fn lex_graphql(text: &str, pal: &Palette) -> Vec<(usize, usize, Color32)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'#' => {
                let end = text[i..].find('\n').map(|p| i + p).unwrap_or(text.len());
                out.push((i, end, pal.comment));
                i = end;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                out.push((start, i, pal.string));
            }
            b'$' => {
                let start = i;
                i += 1;
                while i < bytes.len() && ((bytes[i] as char).is_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push((start, i, pal.key));
            }
            c if (c as char).is_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && ((bytes[i] as char).is_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                let word = &text[start..i];
                if GRAPHQL_KEYWORDS.contains(&word) {
                    out.push((start, i, pal.keyword));
                }
            }
            _ => i += 1,
        }
    }
    out
}
