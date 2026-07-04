//! Application event log tool window (IntelliJ's "Event Log" / Notifications
//! equivalent): every noteworthy app event — workspace opens, runs,
//! transport failures, imports — lands here with a severity and timestamp.

use chrono::{DateTime, Local};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: DateTime<Local>,
    pub level: LogLevel,
    /// Short origin tag ("run", "workspace", "import", "ws", …).
    pub source: &'static str,
    pub message: String,
}

/// Bounded in-memory event log plus the panel's filter state.
pub struct EventLog {
    pub entries: Vec<LogEntry>,
    pub show_info: bool,
    pub show_warn: bool,
    pub show_error: bool,
    pub filter: String,
    pub autoscroll: bool,
}

impl Default for EventLog {
    fn default() -> Self {
        Self { entries: Vec::new(), show_info: true, show_warn: true, show_error: true, filter: String::new(), autoscroll: true }
    }
}

const MAX_ENTRIES: usize = 2_000;

impl EventLog {
    pub fn push(&mut self, level: LogLevel, source: &'static str, message: impl Into<String>) {
        self.entries.push(LogEntry { at: Local::now(), level, source, message: message.into() });
        if self.entries.len() > MAX_ENTRIES {
            let drop = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..drop);
        }
    }

    pub fn info(&mut self, source: &'static str, message: impl Into<String>) {
        self.push(LogLevel::Info, source, message);
    }

    pub fn warn(&mut self, source: &'static str, message: impl Into<String>) {
        self.push(LogLevel::Warn, source, message);
    }

    pub fn error(&mut self, source: &'static str, message: impl Into<String>) {
        self.push(LogLevel::Error, source, message);
    }

    fn visible(&self, entry: &LogEntry) -> bool {
        let level_ok = match entry.level {
            LogLevel::Info => self.show_info,
            LogLevel::Warn => self.show_warn,
            LogLevel::Error => self.show_error,
        };
        if !level_ok {
            return false;
        }
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_lowercase();
        entry.message.to_lowercase().contains(&needle) || entry.source.contains(&needle)
    }
}

/// Render the Log tool window.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let theme = state.theme;
    let (errors, warns) = state
        .log
        .entries
        .iter()
        .fold((0usize, 0usize), |(e, w), en| match en.level {
            LogLevel::Error => (e + 1, w),
            LogLevel::Warn => (e, w + 1),
            LogLevel::Info => (e, w),
        });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Log").strong());
        ui.add_space(6.0);
        ui.toggle_value(&mut state.log.show_info, "Info");
        ui.toggle_value(&mut state.log.show_warn, egui::RichText::new(format!("Warn {warns}")).color(theme.warn_color()));
        ui.toggle_value(&mut state.log.show_error, egui::RichText::new(format!("Error {errors}")).color(theme.error_color()));
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.log.filter)
                .hint_text("Filter…")
                .desired_width(200.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                state.log.entries.clear();
            }
            ui.checkbox(&mut state.log.autoscroll, "Auto-scroll");
        });
    });
    ui.separator();

    let visible: Vec<usize> =
        (0..state.log.entries.len()).filter(|i| state.log.visible(&state.log.entries[*i])).collect();

    let row_h = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if state.log.autoscroll {
        scroll = scroll.stick_to_bottom(true);
    }
    scroll.show_rows(ui, row_h, visible.len(), |ui, range| {
        for &idx in &visible[range] {
            let entry = &state.log.entries[idx];
            let color = match entry.level {
                LogLevel::Info => theme.dim_color(),
                LogLevel::Warn => theme.warn_color(),
                LogLevel::Error => theme.error_color(),
            };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(entry.at.format("%H:%M:%S").to_string())
                        .monospace()
                        .color(theme.dim_color()),
                );
                ui.label(egui::RichText::new(format!("{:5}", entry.level.label())).monospace().color(color).strong());
                ui.label(egui::RichText::new(format!("[{}]", entry.source)).monospace().color(theme.dim_color()));
                ui.label(egui::RichText::new(&entry.message).monospace());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_capped() {
        let mut log = EventLog::default();
        for i in 0..(MAX_ENTRIES + 50) {
            log.info("test", format!("m{i}"));
        }
        assert_eq!(log.entries.len(), MAX_ENTRIES);
        assert_eq!(log.entries[0].message, "m50");
    }

    #[test]
    fn filter_matches_message_and_level() {
        let mut log = EventLog::default();
        log.info("run", "all good");
        log.error("run", "connection refused");
        log.filter = "refused".into();
        let visible: Vec<_> = log.entries.iter().filter(|e| log.visible(e)).collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].level, LogLevel::Error);

        log.filter.clear();
        log.show_error = false;
        let visible = log.entries.iter().filter(|e| log.visible(e)).count();
        assert_eq!(visible, 1);
    }
}
