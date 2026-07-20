//! "Search Everywhere" (bare double `Shift`) / "Search Actions" (`Ctrl+Shift+A`):
//! a centered modal overlay with a fuzzy-matched search box over three
//! sections — open Requests, run Actions and switch Environments.

use std::time::{Duration, Instant};

use egui::{Key, Modal, TextEdit, Ui};

use forge_core::model::Method;
use forge_core::store::{TreeNode, Workspace};

use crate::bridge::Bridge;
use crate::keymap::{self, ActionId};
use crate::state::AppState;
use crate::widgets::method_badge::method_color;

/// Two bare `Shift` presses within this window count as a double-press.
const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(400);
/// Results shown per section.
const MAX_PER_SECTION: usize = 8;

/// Transient state of the Search Everywhere overlay, owned by
/// [`crate::dialogs::DialogManager`].
#[derive(Default)]
pub struct SearchState {
    open_flag: bool,
    /// `true` restricts results to the Actions section (`Ctrl+Shift+A`).
    actions_only: bool,
    query: String,
    /// Index into the flattened, currently-visible result list.
    selected: usize,
    /// Set for one frame after opening, so the query box grabs focus.
    just_opened: bool,
    /// Time of the last completed *bare* Shift tap (see
    /// [`detect_double_shift`]).
    last_tap: Option<Instant>,
    /// A Shift key is currently held down.
    shift_down: bool,
    /// Another key/character was pressed while Shift was held — this press
    /// is a modifier chord (e.g. typing `:`), not a bare tap.
    shift_tainted: bool,
}

impl SearchState {
    /// Open the overlay. `actions_only` restricts it to the Actions section.
    pub fn open(&mut self, actions_only: bool) {
        self.open_flag = true;
        self.actions_only = actions_only;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }

    fn close(&mut self) {
        self.open_flag = false;
        self.query.clear();
    }
}

/// Detect a bare double `Shift` tap within [`DOUBLE_PRESS_WINDOW`].
///
/// A *tap* is Shift pressed and released with no other key or text event in
/// between — so typing shifted characters (`:`, `{`, uppercase letters)
/// never counts, even though it presses Shift. Any interleaved key/text
/// also invalidates a pending first tap, so `A<shift>:<shift>:` while
/// typing a URL cannot accidentally open the dialog.
pub fn detect_double_shift(ctx: &egui::Context, search: &mut SearchState) -> bool {
    let mut triggered = false;
    ctx.input(|input| {
        for event in &input.events {
            match event {
                egui::Event::Key {
                    key: Key::ShiftLeft | Key::ShiftRight,
                    pressed,
                    repeat: false,
                    modifiers,
                    ..
                } => {
                    let bare = !modifiers.ctrl && !modifiers.alt && !modifiers.command;
                    if *pressed {
                        search.shift_down = true;
                        search.shift_tainted = !bare;
                    } else if search.shift_down {
                        search.shift_down = false;
                        if search.shift_tainted {
                            search.last_tap = None;
                            continue;
                        }
                        let now = Instant::now();
                        match search.last_tap {
                            Some(prev) if now.duration_since(prev) <= DOUBLE_PRESS_WINDOW => {
                                triggered = true;
                                search.last_tap = None;
                            }
                            _ => search.last_tap = Some(now),
                        }
                    }
                }
                egui::Event::Key { .. } | egui::Event::Text(_) => {
                    // Any other key or typed character: taints a held Shift
                    // and cancels a pending first tap.
                    if search.shift_down {
                        search.shift_tainted = true;
                    }
                    search.last_tap = None;
                }
                _ => {}
            }
        }
    });
    triggered
}

/// One flattened, selectable result row.
enum Item {
    Request {
        rel_id: String,
        method: Method,
        label: String,
    },
    Action {
        id: ActionId,
        title: &'static str,
        shortcut: Option<String>,
    },
    Environment {
        name: String,
    },
}

/// Case-insensitive subsequence fuzzy scorer: returns `None` if `query`'s
/// characters don't all appear, in order, in `candidate`; otherwise a score
/// where higher is a better match. Consecutive matches and matches at the
/// start of a word are rewarded; gaps between matched characters are
/// penalized.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();

    let mut score = 0i32;
    let mut cursor = 0usize;
    let mut last_match: Option<usize> = None;
    let mut run = 0i32;

    for &qc in &q {
        let idx = (cursor..c.len()).find(|&i| c[i] == qc)?;

        if idx == 0 || matches!(c[idx - 1], ' ' | '-' | '_' | '/' | '.') {
            score += 10;
        }
        if let Some(last) = last_match {
            let gap = idx as i32 - last as i32 - 1;
            if gap == 0 {
                run += 1;
                score += 5 + run;
            } else {
                run = 0;
                score -= gap;
            }
        }
        score += 1;
        last_match = Some(idx);
        cursor = idx + 1;
    }

    // Slight preference for tighter (shorter) candidates among equal matches.
    score -= (c.len() as i32 - q.len() as i32).max(0) / 8;
    Some(score)
}

/// Collect `(rel_id, method, "Collection / ... / name")` for every request in
/// the workspace, depth-first.
fn request_entries(workspace: &Workspace) -> Vec<(String, Method, String)> {
    let mut out = Vec::new();
    for col in &workspace.collections {
        collect(&col.children, workspace, &col.meta.name, &mut out);
    }
    out
}

fn collect(
    children: &[TreeNode],
    workspace: &Workspace,
    prefix: &str,
    out: &mut Vec<(String, Method, String)>,
) {
    for child in children {
        match child {
            TreeNode::Request(r) => {
                out.push((
                    workspace.rel_id(&r.file),
                    r.def.method,
                    format!("{prefix} / {}", r.def.name),
                ));
            }
            TreeNode::Folder(f) => {
                let name = child.display_name();
                collect(&f.children, workspace, &format!("{prefix} / {name}"), out);
            }
        }
    }
}

/// Render the overlay if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    if !state.dialogs.search.open_flag {
        return;
    }

    let query_lower = state.dialogs.search.query.clone();

    let mut request_items: Vec<Item> = Vec::new();
    let mut env_items: Vec<Item> = Vec::new();

    if !state.dialogs.search.actions_only {
        if let Some(ws) = &state.workspace {
            let mut scored: Vec<(i32, Item)> = request_entries(ws)
                .into_iter()
                .filter_map(|(rel_id, method, label)| {
                    fuzzy_score(&query_lower, &label).map(|s| {
                        (
                            s,
                            Item::Request {
                                rel_id,
                                method,
                                label,
                            },
                        )
                    })
                })
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            request_items = scored
                .into_iter()
                .take(MAX_PER_SECTION)
                .map(|(_, i)| i)
                .collect();

            let mut scored: Vec<(i32, Item)> = ws
                .environments
                .iter()
                .filter_map(|e| {
                    fuzzy_score(&query_lower, &e.env.name).map(|s| {
                        (
                            s,
                            Item::Environment {
                                name: e.env.name.clone(),
                            },
                        )
                    })
                })
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            env_items = scored
                .into_iter()
                .take(MAX_PER_SECTION)
                .map(|(_, i)| i)
                .collect();
        }
    }

    let mut scored: Vec<(i32, Item)> = keymap::ACTIONS
        .iter()
        .filter_map(|a| {
            fuzzy_score(&query_lower, a.title).map(|s| {
                let shortcut = a.shortcut.map(|sc| ctx.format_shortcut(&sc));
                (
                    s,
                    Item::Action {
                        id: a.id,
                        title: a.title,
                        shortcut,
                    },
                )
            })
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let action_items: Vec<Item> = scored
        .into_iter()
        .take(MAX_PER_SECTION)
        .map(|(_, i)| i)
        .collect();

    let total = request_items.len() + action_items.len() + env_items.len();
    if state.dialogs.search.selected >= total.max(1) {
        state.dialogs.search.selected = total.saturating_sub(1);
    }

    let mut close_requested = false;
    let mut activate: Option<usize> = None;

    let modal = Modal::new(egui::Id::new("search-everywhere")).show(ctx, |ui| {
        ui.set_min_width(480.0);
        ui.set_max_width(560.0);

        let title = if state.dialogs.search.actions_only {
            "Search Actions"
        } else {
            "Search Everywhere"
        };
        ui.heading(title);
        ui.add_space(4.0);

        let response = ui.add(
            TextEdit::singleline(&mut state.dialogs.search.query)
                .desired_width(f32::INFINITY)
                .hint_text("Type to search..."),
        );
        if state.dialogs.search.just_opened {
            response.request_focus();
            state.dialogs.search.just_opened = false;
        }
        if response.changed() {
            state.dialogs.search.selected = 0;
        }

        ui.input(|i| {
            if i.key_pressed(Key::ArrowDown) && total > 0 {
                state.dialogs.search.selected = (state.dialogs.search.selected + 1) % total;
            }
            if i.key_pressed(Key::ArrowUp) && total > 0 {
                state.dialogs.search.selected = (state.dialogs.search.selected + total - 1) % total;
            }
            if i.key_pressed(Key::Enter) && total > 0 {
                activate = Some(state.dialogs.search.selected);
            }
        });

        ui.add_space(6.0);
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("search-sa-1")
            .max_height(360.0)
            .show(ui, |ui| {
                let mut flat_idx = 0usize;
                if !request_items.is_empty() {
                    section_header(ui, "Requests");
                    for item in &request_items {
                        if row(ui, item, flat_idx == state.dialogs.search.selected) {
                            activate = Some(flat_idx);
                        }
                        flat_idx += 1;
                    }
                }
                if !action_items.is_empty() {
                    section_header(ui, "Actions");
                    for item in &action_items {
                        if row(ui, item, flat_idx == state.dialogs.search.selected) {
                            activate = Some(flat_idx);
                        }
                        flat_idx += 1;
                    }
                }
                if !env_items.is_empty() {
                    section_header(ui, "Environments");
                    for item in &env_items {
                        if row(ui, item, flat_idx == state.dialogs.search.selected) {
                            activate = Some(flat_idx);
                        }
                        flat_idx += 1;
                    }
                }
                if flat_idx == 0 {
                    ui.add_space(8.0);
                    ui.weak("No matches.");
                }
            });
    });

    if modal.should_close() {
        close_requested = true;
    }

    if let Some(idx) = activate {
        let mut all: Vec<Item> = Vec::new();
        all.extend(request_items);
        all.extend(action_items);
        all.extend(env_items);
        if let Some(item) = all.into_iter().nth(idx) {
            match item {
                Item::Request { rel_id, .. } => {
                    let def = state
                        .workspace
                        .as_ref()
                        .and_then(|ws| ws.find_request(&rel_id).map(|n| n.def.clone()));
                    if let Some(def) = def {
                        state.open_tab(rel_id, def);
                    }
                }
                Item::Action { id, .. } => super::dispatch_action(state, bridge, id),
                Item::Environment { name } => state.active_env = Some(name),
            }
        }
        close_requested = true;
    }

    if close_requested {
        state.dialogs.search.close();
    }
}

fn section_header(ui: &mut Ui, label: &str) {
    ui.add_space(4.0);
    ui.weak(label);
}

/// Render one result row; returns `true` if it was activated (clicked).
fn row(ui: &mut Ui, item: &Item, selected: bool) -> bool {
    let frame = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(6, 3))
        .fill(if selected {
            ui.visuals().selection.bg_fill.gamma_multiply(0.35)
        } else {
            egui::Color32::TRANSPARENT
        });
    let mut clicked = false;
    frame.show(ui, |ui| {
        ui.horizontal(|ui| match item {
            Item::Request { method, label, .. } => {
                ui.label(
                    egui::RichText::new(method.as_str())
                        .color(method_color(*method))
                        .monospace()
                        .strong()
                        .size(13.0),
                );
                if ui.selectable_label(false, label).clicked() {
                    clicked = true;
                }
            }
            Item::Action {
                title, shortcut, ..
            } => {
                if ui.selectable_label(false, *title).clicked() {
                    clicked = true;
                }
                if let Some(sc) = shortcut {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(sc)
                    });
                }
            }
            Item::Environment { name } => {
                if ui
                    .selectable_label(false, format!("\u{1F30D} {name}"))
                    .clicked()
                {
                    clicked = true;
                }
            }
        });
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_scores_higher_than_scattered() {
        let tight = fuzzy_score("get", "get").unwrap();
        // No word-boundary characters around the scattered matches, so this
        // only exercises the gap penalty, not the word-start bonus.
        let scattered = fuzzy_score("get", "xxgxxexxtxx").unwrap();
        assert!(tight > scattered, "tight={tight} scattered={scattered}");
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_score("xyz", "abc"), None);
        assert_eq!(fuzzy_score("get", "teg"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(fuzzy_score("GET", "get pets").is_some());
        assert!(fuzzy_score("get", "GET Pets").is_some());
        assert_eq!(fuzzy_score("GeT", "get"), fuzzy_score("get", "GET"));
    }

    #[test]
    fn consecutive_run_beats_same_length_with_gaps() {
        let consecutive = fuzzy_score("abc", "abcxxxx").unwrap();
        let gappy = fuzzy_score("abc", "axbxcxxx").unwrap();
        assert!(
            consecutive > gappy,
            "consecutive={consecutive} gappy={gappy}"
        );
    }

    #[test]
    fn word_start_bonus_ranks_prefix_first() {
        let prefix = fuzzy_score("pet", "pet store").unwrap();
        let mid_word = fuzzy_score("pet", "car pet-eria").unwrap();
        assert!(prefix >= mid_word, "prefix={prefix} mid_word={mid_word}");
    }

    #[test]
    fn empty_query_matches_everything_with_zero_score() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
