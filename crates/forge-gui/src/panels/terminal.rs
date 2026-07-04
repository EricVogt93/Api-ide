//! Embedded terminal tool window: runs shell commands (`sh -c`) with the
//! workspace root as working directory, streaming stdout/stderr live into a
//! scrollback with basic ANSI SGR color support.
//!
//! This is a command runner, not a full PTY — interactive TUI programs
//! (vim, htop) are out of scope; `curl`, `forge run`, `git` and friends work.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};

use crate::state::AppState;
use crate::theme::ThemeKind;

/// One styled fragment of a terminal line.
#[derive(Debug, Clone, PartialEq)]
pub struct TermSpan {
    pub text: String,
    /// ANSI color index 0-15, `None` = default foreground.
    pub color: Option<u8>,
    pub bold: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TermLine {
    pub spans: Vec<TermSpan>,
    /// Line came from stderr (tinted red-ish when no explicit color is set).
    pub stderr: bool,
}

pub enum TermEvent {
    Line(TermLine),
    Exited(Option<i32>),
}

#[derive(Default)]
pub struct TerminalState {
    pub lines: Vec<TermLine>,
    pub input: String,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    rx: Option<Receiver<TermEvent>>,
    child: Option<Arc<Mutex<Child>>>,
    pub running: bool,
    pub autoscroll: bool,
}

const MAX_LINES: usize = 5_000;

impl TerminalState {
    fn push_line(&mut self, line: TermLine) {
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(0..drop);
        }
    }

    fn push_plain(&mut self, text: impl Into<String>, color: Option<u8>) {
        self.push_line(TermLine {
            spans: vec![TermSpan { text: text.into(), color, bold: false }],
            stderr: false,
        });
    }

    /// Poll pending output from the reader threads.
    fn drain(&mut self) {
        let mut exited = None;
        let mut pending: Vec<TermLine> = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(evt) = rx.try_recv() {
                match evt {
                    TermEvent::Line(line) => pending.push(line),
                    TermEvent::Exited(code) => exited = Some(code),
                }
            }
        }
        for line in pending {
            self.push_line(line);
        }
        if let Some(code) = exited {
            let msg = match code {
                Some(0) => "process finished".to_string(),
                Some(c) => format!("process finished with exit code {c}"),
                None => "process terminated".to_string(),
            };
            self.push_plain(msg, Some(8));
            self.running = false;
            self.rx = None;
            self.child = None;
        }
    }

    fn kill(&mut self) {
        if let Some(child) = &self.child {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
        }
    }

    fn exec(&mut self, cmd: String, cwd: Option<PathBuf>, ctx: egui::Context) {
        let prompt = format!("$ {cmd}");
        self.push_plain(prompt, Some(14));
        self.history.retain(|h| *h != cmd);
        self.history.push(cmd.clone());
        self.history_pos = None;

        let mut command = Command::new("sh");
        command.arg("-c").arg(&cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }

        match command.spawn() {
            Ok(mut child) => {
                let (tx, rx) = std::sync::mpsc::channel::<TermEvent>();
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let child = Arc::new(Mutex::new(child));

                if let Some(out) = stdout {
                    spawn_reader(out, false, tx.clone(), ctx.clone());
                }
                if let Some(err) = stderr {
                    spawn_reader(err, true, tx.clone(), ctx.clone());
                }
                // Waiter thread: reports the exit code once both pipes close.
                let waiter_child = Arc::clone(&child);
                std::thread::spawn(move || {
                    let code = loop {
                        match waiter_child.lock().map(|mut c| c.try_wait()) {
                            Ok(Ok(Some(status))) => break status.code(),
                            Ok(Ok(None)) => std::thread::sleep(std::time::Duration::from_millis(60)),
                            _ => break None,
                        }
                    };
                    let _ = tx.send(TermEvent::Exited(code));
                    ctx.request_repaint();
                });

                self.rx = Some(rx);
                self.child = Some(child);
                self.running = true;
            }
            Err(e) => self.push_plain(format!("failed to spawn: {e}"), Some(9)),
        }
    }
}

fn spawn_reader(pipe: impl std::io::Read + Send + 'static, stderr: bool, tx: Sender<TermEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let mut parsed = parse_ansi_line(&line);
            parsed.stderr = stderr;
            if tx.send(TermEvent::Line(parsed)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
}

/// Parse one line of terminal output into styled spans, honoring the common
/// SGR color codes (30-37/90-97 foreground, 1 bold, 0 reset) and silently
/// stripping every other escape sequence.
pub fn parse_ansi_line(line: &str) -> TermLine {
    let mut spans: Vec<TermSpan> = Vec::new();
    let mut current = TermSpan { text: String::new(), color: None, bold: false };
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            current.text.push(c);
            continue;
        }
        // Escape sequence. Only CSI ... 'm' (SGR) is interpreted.
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut terminator = None;
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                terminator = Some(c);
                break;
            }
            params.push(c);
        }
        if terminator != Some('m') {
            continue;
        }
        if !current.text.is_empty() {
            spans.push(current.clone());
            current.text.clear();
        }
        for code in params.split(';') {
            match code.parse::<u8>().unwrap_or(0) {
                0 => {
                    current.color = None;
                    current.bold = false;
                }
                1 => current.bold = true,
                n @ 30..=37 => current.color = Some(n - 30),
                39 => current.color = None,
                n @ 90..=97 => current.color = Some(n - 90 + 8),
                _ => {}
            }
        }
    }
    if !current.text.is_empty() {
        spans.push(current);
    }
    TermLine { spans, stderr: false }
}

/// Map an ANSI color index to a theme-appropriate RGB.
fn ansi_color(idx: u8, theme: ThemeKind) -> Color32 {
    let dark = theme == ThemeKind::Darcula;
    match idx {
        0 => if dark { Color32::from_rgb(0x86, 0x8A, 0x91) } else { Color32::from_rgb(0x00, 0x00, 0x00) },
        1 | 9 => theme.error_color(),
        2 | 10 => theme.ok_color(),
        3 | 11 => theme.warn_color(),
        4 | 12 => theme.accent_color(),
        5 | 13 => Color32::from_rgb(0xC5, 0x7B, 0xD8),
        6 | 14 => Color32::from_rgb(0x33, 0xC0, 0xC0),
        8 => theme.dim_color(),
        _ => if dark { Color32::from_rgb(0xDF, 0xE1, 0xE5) } else { Color32::from_rgb(0x27, 0x28, 0x2E) },
    }
}

/// Render the Terminal tool window.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    state.terminal.drain();
    let theme = state.theme;
    let cwd = state.workspace.as_ref().map(|w| w.root.clone());

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Terminal").strong());
        let cwd_label = cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        ui.label(egui::RichText::new(cwd_label).color(theme.dim_color()).small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                state.terminal.lines.clear();
            }
            if state.terminal.running && ui.button(egui::RichText::new("Stop").color(theme.error_color())).clicked() {
                state.terminal.kill();
            }
            ui.checkbox(&mut state.terminal.autoscroll, "Auto-scroll");
        });
    });
    ui.separator();

    let input_height = 30.0;
    let avail = ui.available_height() - input_height;
    let font = FontId::monospace(13.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;

    let bg = theme.editor_bg();
    egui::Frame::new().fill(bg).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        ui.set_min_height(avail - 12.0);
        let mut scroll = egui::ScrollArea::vertical().max_height(avail - 12.0).auto_shrink([false, false]);
        if state.terminal.autoscroll {
            scroll = scroll.stick_to_bottom(true);
        }
        let lines = &state.terminal.lines;
        scroll.show_rows(ui, row_h, lines.len(), |ui, range| {
            for line in &lines[range] {
                let mut job = LayoutJob::default();
                for span in &line.spans {
                    let color = match span.color {
                        Some(idx) => ansi_color(idx, theme),
                        None if line.stderr => theme.error_color(),
                        None => ansi_color(255, theme),
                    };
                    let mut fmt = TextFormat { font_id: font.clone(), color, ..Default::default() };
                    if span.bold {
                        fmt.underline = egui::Stroke::NONE;
                        fmt.color = color.gamma_multiply(1.15);
                    }
                    job.append(&span.text, 0.0, fmt);
                }
                if job.sections.is_empty() {
                    job.append(" ", 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
                }
                ui.label(job);
            }
        });
    });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("$").color(theme.accent_color()).monospace().strong());
        let edit = egui::TextEdit::singleline(&mut state.terminal.input)
            .font(egui::TextStyle::Monospace)
            .hint_text("Run a command… (Enter to execute)")
            .desired_width(f32::INFINITY);
        let resp = ui.add(edit);

        // Up/Down history navigation while the input has focus.
        if resp.has_focus() {
            let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
            if up || down {
                navigate_history(&mut state.terminal, up);
            }
        }

        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let cmd = state.terminal.input.trim().to_string();
            state.terminal.input.clear();
            if !cmd.is_empty() {
                let ctx = ui.ctx().clone();
                state.terminal.exec(cmd, cwd, ctx);
            }
            resp.request_focus();
        }
    });
}

fn navigate_history(term: &mut TerminalState, up: bool) {
    if term.history.is_empty() {
        return;
    }
    let pos = match (term.history_pos, up) {
        (None, true) => Some(term.history.len() - 1),
        (None, false) => None,
        (Some(0), true) => Some(0),
        (Some(p), true) => Some(p - 1),
        (Some(p), false) if p + 1 < term.history.len() => Some(p + 1),
        (Some(_), false) => None,
    };
    term.history_pos = pos;
    term.input = pos.map(|p| term.history[p].clone()).unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_default_span() {
        let line = parse_ansi_line("hello world");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hello world");
        assert_eq!(line.spans[0].color, None);
    }

    #[test]
    fn sgr_colors_split_spans() {
        let line = parse_ansi_line("\u{1b}[31mred\u{1b}[0m plain \u{1b}[92mgreen\u{1b}[0m");
        let texts: Vec<&str> = line.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["red", " plain ", "green"]);
        assert_eq!(line.spans[0].color, Some(1));
        assert_eq!(line.spans[1].color, None);
        assert_eq!(line.spans[2].color, Some(10));
    }

    #[test]
    fn non_sgr_escapes_are_stripped() {
        let line = parse_ansi_line("\u{1b}[2Kcleared\u{1b}[1;31m!");
        let joined: String = line.spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "cleared!");
        assert!(line.spans.last().unwrap().bold);
    }

    #[test]
    fn history_navigation_wraps_sensibly() {
        let mut term = TerminalState { history: vec!["a".into(), "b".into()], ..Default::default() };
        navigate_history(&mut term, true);
        assert_eq!(term.input, "b");
        navigate_history(&mut term, true);
        assert_eq!(term.input, "a");
        navigate_history(&mut term, true);
        assert_eq!(term.input, "a");
        navigate_history(&mut term, false);
        assert_eq!(term.input, "b");
        navigate_history(&mut term, false);
        assert_eq!(term.input, "");
    }

    #[test]
    fn scrollback_is_capped() {
        let mut term = TerminalState::default();
        for i in 0..(MAX_LINES + 100) {
            term.push_plain(format!("line {i}"), None);
        }
        assert_eq!(term.lines.len(), MAX_LINES);
        assert_eq!(term.lines[0].spans[0].text, "line 100");
    }
}
