//! The empty-workspace landing pane: rendered in the central panel instead
//! of an empty editor area whenever `state.workspace` is `None`.

use std::path::{Path, PathBuf};

use egui::Ui;
use serde::{Deserialize, Serialize};

use forge_core::store::Workspace;

use crate::state::{AppState, StatusMessage};

/// Recent workspaces are capped at this many entries, most-recent first.
const MAX_RECENTS: usize = 10;

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentWorkspaces {
    #[serde(default)]
    paths: Vec<String>,
}

fn recents_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("forge")
            .join("recent.json"),
    )
}

fn load_recents_from(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str::<RecentWorkspaces>(&s).ok())
        .map(|r| r.paths)
        .unwrap_or_default()
}

fn load_recents() -> Vec<String> {
    recents_file()
        .map(|f| load_recents_from(&f))
        .unwrap_or_default()
}

/// Push `path` to the front of `recents`, removing any earlier occurrence
/// first, then cap at [`MAX_RECENTS`]. Pure logic, unit-tested below.
fn add_recent(recents: &mut Vec<String>, path: String) {
    recents.retain(|p| p != &path);
    recents.insert(0, path);
    recents.truncate(MAX_RECENTS);
}

/// Record `path` as the most recently used workspace, persisting to
/// `$HOME/.config/forge/recent.json`. Best-effort: a missing `HOME` or an
/// unwritable disk silently no-ops — recents are a convenience, not
/// load-bearing state.
pub fn remember_recent(path: &Path) {
    let Some(file) = recents_file() else { return };
    let mut recents = load_recents_from(&file);
    add_recent(&mut recents, path.display().to_string());
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&RecentWorkspaces { paths: recents }) {
        let _ = std::fs::write(&file, json);
    }
}

/// Render the centered welcome pane.
pub fn show(ui: &mut Ui, state: &mut AppState) {
    let recents = load_recents();
    let mut open_path: Option<PathBuf> = None;

    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.15).min(120.0));
        ui.heading("Forge");
        ui.weak("An IntelliJ-style API testing IDE.");
        ui.add_space(24.0);

        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 264.0).max(0.0) / 2.0);
            if ui
                .add_sized([120.0, 32.0], egui::Button::new("Open Workspace..."))
                .clicked()
            {
                super::open_workspace(state);
            }
            ui.add_space(8.0);
            if ui
                .add_sized([120.0, 32.0], egui::Button::new("New Workspace..."))
                .clicked()
            {
                super::new_workspace(state);
            }
        });

        if !recents.is_empty() {
            ui.add_space(28.0);
            ui.label("Recent workspaces:");
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.set_max_width(460.0);
                for p in &recents {
                    if ui.selectable_label(false, p).clicked() {
                        open_path = Some(PathBuf::from(p));
                    }
                }
            });
        }
    });

    if let Some(path) = open_path {
        open_recent(state, &path);
    }
}

fn open_recent(state: &mut AppState, path: &Path) {
    match Workspace::load(path) {
        Ok(ws) => {
            state.pending_workspace = Some(ws);
            state.status = Some(StatusMessage::info(format!("Opened {}", path.display())));
            remember_recent(path);
        }
        Err(e) => state.status = Some(StatusMessage::error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_recent_inserts_at_front() {
        let mut recents = vec!["a".to_string(), "b".to_string()];
        add_recent(&mut recents, "c".to_string());
        assert_eq!(recents, vec!["c", "a", "b"]);
    }

    #[test]
    fn add_recent_dedupes_moving_existing_to_front() {
        let mut recents = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        add_recent(&mut recents, "b".to_string());
        assert_eq!(recents, vec!["b", "a", "c"]);
    }

    #[test]
    fn add_recent_caps_at_max() {
        let mut recents: Vec<String> = (0..MAX_RECENTS).map(|i| i.to_string()).collect();
        add_recent(&mut recents, "new".to_string());
        assert_eq!(recents.len(), MAX_RECENTS);
        assert_eq!(recents[0], "new");
        // The oldest entry (pushed furthest back) is evicted.
        assert!(!recents.contains(&(MAX_RECENTS - 1).to_string()));
    }

    #[test]
    fn add_recent_reinserting_existing_does_not_grow_list() {
        let mut recents: Vec<String> = (0..MAX_RECENTS).map(|i| i.to_string()).collect();
        add_recent(&mut recents, "5".to_string());
        assert_eq!(recents.len(), MAX_RECENTS);
        assert_eq!(recents[0], "5");
    }
}
