//! Lightweight in-product onboarding. The tour deliberately points at the
//! real shell instead of duplicating it in a wizard, so it stays useful as
//! panels and shortcuts evolve.

use crate::state::{AppState, BottomTool};
use crate::theme::icons;

const STEP_COUNT: usize = 10;

#[derive(Default)]
pub struct TourState {
    pub open: bool,
    step: usize,
}

impl TourState {
    pub fn start(&mut self) {
        self.step = 0;
        self.open = true;
    }

    fn next(&mut self) {
        if self.step + 1 < STEP_COUNT {
            self.step += 1;
        } else {
            self.open = false;
        }
    }

    fn previous(&mut self) {
        self.step = self.step.saturating_sub(1);
    }
}

#[derive(Clone, Copy)]
enum Target {
    Whole,
    Project,
    Catalog,
    Editor,
    RightTools,
    Results,
    BottomTools,
    Menu,
    Status,
}

struct Step {
    title: &'static str,
    text: &'static str,
    action: &'static str,
    target: Target,
}

const STEPS: [Step; STEP_COUNT] = [
    Step {
        title: "Welcome to ApiWright",
        text: "ApiWright keeps requests, reusable behavior and project metadata in ordinary files. The demo workspace is safe to explore and every modern request has an offline mock.",
        action: "Use Next or the arrow keys to follow the main workflow.",
        target: Target::Whole,
    },
    Step {
        title: "Project explorer",
        text: "Folders organize stories. Requests, environments, assets and sequences use distinct icons; Git state and inherited Jira links remain visible beside them.",
        action: "Right-click a folder or request for run, properties, export and Git actions.",
        target: Target::Project,
    },
    Step {
        title: "Reusable catalog",
        text: "Search by intent, select a built-in or project asset, fill its typed parameters and insert it. Assertions and hooks are stored in clean sidecar files.",
        action: "Open Create pet and compare its request, Assertions and Hooks tabs.",
        target: Target::Catalog,
    },
    Step {
        title: "Request editor",
        text: "Edit JSON with validation, OpenAPI completion, syntax highlighting, formatting, a minimap and Ctrl+mouse-wheel zoom. Run uses the selected environment and mode.",
        action: "Press Tab on an OpenAPI suggestion or run a demo request in mock mode.",
        target: Target::Editor,
    },
    Step {
        title: "OpenAPI and generators",
        text: "The right tool window groups OpenAPI operations, tracks coverage and generates contract, API and k6 suites. The AI Advisor can receive the active file and optional response.",
        action: "Use the icon tabs to switch tools; collapse the window with its chevron.",
        target: Target::RightTools,
    },
    Step {
        title: "Response and test details",
        text: "Response, Assertions, Hooks, Auth, Runtime, Trace and Diagnostics stay separated. Drag the splitter or zoom these panes independently with Ctrl+mouse wheel.",
        action: "Run List pets, then inspect its formatted response and assertion results.",
        target: Target::Results,
    },
    Step {
        title: "Tool windows",
        text: "Run results, Problems, Terminal and History are one click away; More contains logs, console, cookies and variables. Clicking an active tab collapses it.",
        action: "Open History after a run or Terminal for project-local commands.",
        target: Target::BottomTools,
    },
    Step {
        title: "Menus and focus modes",
        text: "File contains import and export, Run executes the current scope, and View controls every bar. Zen mode hides chrome until the screen edge is hovered.",
        action: "Return to this guide any time with View → User tour.",
        target: Target::Menu,
    },
    Step {
        title: "Git, worktrees and timing",
        text: "The status bar switches branches, creates worktrees, reports readiness, execution time and the installed ApiWright version.",
        action: "Click the branch name for repository actions.",
        target: Target::Status,
    },
    Step {
        title: "Explore the demo",
        text: "Start with List pets, Create pet, the data matrix and the auth folder. Then inspect their sidecars and project assets or generate a suite from the Petstore spec.",
        action: "Everything is file-backed, so changes stay reviewable in Git.",
        target: Target::Whole,
    },
];

pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.dialogs.tour.open {
        return;
    }

    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.dialogs.tour.open = false;
        return;
    }
    if ctx.input(|input| input.key_pressed(egui::Key::ArrowLeft)) {
        state.dialogs.tour.previous();
    }
    if ctx.input(|input| input.key_pressed(egui::Key::ArrowRight)) {
        state.dialogs.tour.next();
    }
    if !state.dialogs.tour.open {
        return;
    }

    let step_index = state.dialogs.tour.step.min(STEP_COUNT - 1);
    prepare_shell(state, step_index);
    let step = &STEPS[step_index];
    let screen = ctx.content_rect();
    let target = target_rect(screen, step.target);
    let accent = state.theme.accent_color();
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("user-tour-highlight"),
    ))
    .rect_stroke(
        target,
        8,
        egui::Stroke::new(2.0, accent),
        egui::StrokeKind::Outside,
    );

    let card_size = egui::vec2(390.0, 236.0);
    let card_pos = card_position(screen, target, step.target, card_size);
    let mut go_back = false;
    let mut go_next = false;
    let mut close = false;
    egui::Window::new("User tour")
        .id(egui::Id::new("user-tour-card"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_pos(card_pos)
        .fixed_size(card_size)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("GUIDE  {} / {STEP_COUNT}", step_index + 1))
                        .small()
                        .strong()
                        .color(accent),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui
                        .small_button(icons::CLOSE)
                        .on_hover_text("Close the user tour")
                        .clicked();
                });
            });
            ui.add_space(6.0);
            ui.heading(step.title);
            ui.add_space(6.0);
            ui.label(step.text);
            ui.add_space(8.0);
            egui::Frame::NONE
                .fill(ui.visuals().widgets.inactive.weak_bg_fill)
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(step.action).strong());
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                go_back = ui
                    .add_enabled(step_index > 0, egui::Button::new("← Back"))
                    .on_hover_text("Show the previous tour step")
                    .clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if step_index + 1 == STEP_COUNT {
                        "Finish"
                    } else {
                        "Next →"
                    };
                    go_next = ui
                        .add(crate::theme::primary_button(label, accent))
                        .on_hover_text(if step_index + 1 == STEP_COUNT {
                            "Finish the user tour"
                        } else {
                            "Show the next tour step"
                        })
                        .clicked();
                });
            });
        });

    if close {
        state.dialogs.tour.open = false;
    } else if go_back {
        state.dialogs.tour.previous();
    } else if go_next {
        state.dialogs.tour.next();
    }
}

fn prepare_shell(state: &mut AppState, step: usize) {
    state.zen_mode = false;
    state.show_activity_bar = true;
    state.show_status_bar = true;
    state.show_bottom_bar = true;
    if matches!(step, 1 | 2) {
        state.show_assets = true;
    }
    if step == 4 {
        state.dialogs.v1_editor.reveal_right_tools();
    }
    if step == 6 {
        state.bottom_tool = Some(BottomTool::Run);
    }
}

fn target_rect(screen: egui::Rect, target: Target) -> egui::Rect {
    let inset = screen.shrink(8.0);
    match target {
        Target::Whole => inset,
        Target::Project => egui::Rect::from_min_max(
            egui::pos2(inset.left(), inset.top() + 55.0),
            egui::pos2(
                (inset.left() + 375.0).min(inset.right()),
                inset.bottom() - 26.0,
            ),
        ),
        Target::Catalog => egui::Rect::from_min_max(
            egui::pos2(
                (inset.left() + 375.0).min(inset.right()),
                inset.top() + 95.0,
            ),
            egui::pos2(
                (inset.left() + 710.0).min(inset.right()),
                inset.bottom() - 185.0,
            ),
        ),
        Target::Editor => egui::Rect::from_min_max(
            egui::pos2(
                (inset.left() + 710.0).min(inset.right()),
                inset.top() + 55.0,
            ),
            egui::pos2(
                (inset.right() - 300.0).max(inset.left()),
                inset.bottom() - 185.0,
            ),
        ),
        Target::RightTools => egui::Rect::from_min_max(
            egui::pos2(
                (inset.right() - 305.0).max(inset.left()),
                inset.top() + 55.0,
            ),
            egui::pos2(inset.right(), inset.bottom() - 26.0),
        ),
        Target::Results => egui::Rect::from_min_max(
            egui::pos2(
                (inset.left() + 375.0).min(inset.right()),
                (inset.bottom() - 360.0).max(inset.top()),
            ),
            egui::pos2(
                (inset.right() - 300.0).max(inset.left()),
                inset.bottom() - 60.0,
            ),
        ),
        Target::BottomTools => egui::Rect::from_min_max(
            egui::pos2(inset.left(), inset.bottom() - 62.0),
            egui::pos2(inset.right(), inset.bottom() - 26.0),
        ),
        Target::Menu => egui::Rect::from_min_max(
            egui::pos2(inset.left(), inset.top()),
            egui::pos2(inset.right(), inset.top() + 48.0),
        ),
        Target::Status => egui::Rect::from_min_max(
            egui::pos2(inset.left(), inset.bottom() - 26.0),
            egui::pos2(inset.right(), inset.bottom()),
        ),
    }
}

fn card_position(
    screen: egui::Rect,
    target: egui::Rect,
    kind: Target,
    card_size: egui::Vec2,
) -> egui::Pos2 {
    let margin = 18.0;
    let desired = match kind {
        Target::Project | Target::Catalog => {
            egui::pos2(target.right() + margin, target.top() + 24.0)
        }
        Target::RightTools => egui::pos2(target.left() - card_size.x - margin, target.top() + 24.0),
        Target::Results | Target::BottomTools | Target::Status => egui::pos2(
            screen.center().x - card_size.x / 2.0,
            target.top() - card_size.y - margin,
        ),
        Target::Menu => egui::pos2(
            screen.center().x - card_size.x / 2.0,
            target.bottom() + margin,
        ),
        Target::Whole | Target::Editor => egui::pos2(
            screen.center().x - card_size.x / 2.0,
            screen.center().y - card_size.y / 2.0,
        ),
    };
    egui::pos2(
        desired.x.clamp(
            screen.left() + margin,
            (screen.right() - card_size.x - margin).max(screen.left() + margin),
        ),
        desired.y.clamp(
            screen.top() + margin,
            (screen.bottom() - card_size.y - margin).max(screen.top() + margin),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_stays_in_bounds_and_finishes() {
        let mut tour = TourState::default();
        tour.start();
        tour.previous();
        assert_eq!(tour.step, 0);
        for _ in 0..STEP_COUNT - 1 {
            tour.next();
        }
        assert!(tour.open);
        assert_eq!(tour.step, STEP_COUNT - 1);
        tour.next();
        assert!(!tour.open);
    }
}
