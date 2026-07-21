//! File-explorer view of a reqv1 project. The filesystem stays the source of
//! truth; requests can be grouped in arbitrary story folders.

use std::path::{Component, Path, PathBuf};

use egui::{RichText, Ui};
use forge_core::reqv1::{AssetKind, BundleFormat, ProjectIndex};

use crate::bridge::Bridge;
use crate::git::{FileStatus, GitStatus};
use crate::panels::collections::{self, PendingAction};
use crate::state::{AppState, StatusMessage};
use crate::theme::icons;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ProjectView {
    #[default]
    Files,
    Git,
}

#[derive(Debug, Clone)]
struct ProjectNode {
    path: PathBuf,
    directory: bool,
    children: Vec<ProjectNode>,
    ticket: Option<forge_core::reqv1::TicketLink>,
}

enum ProjectAction {
    NewRequest(PathBuf),
    NewFolder(PathBuf),
    AddFiles(PathBuf),
    Beautify(PathBuf),
    Export(PathBuf, BundleFormat),
    Import(PathBuf),
    Properties(PathBuf),
    Open(PathBuf),
}

#[derive(Default)]
pub struct AssetsState {
    /// Project root currently indexed.
    root: Option<PathBuf>,
    index: Option<ProjectIndex>,
    tree: Vec<ProjectNode>,
    error: Option<String>,
    collapsed: std::collections::HashSet<PathBuf>,
    selected_dir: Option<PathBuf>,
    view: ProjectView,
    /// Base dir for `suggest_ref` — the active request's directory, so a
    /// copied relative ref is correct for what the author is editing.
    base_dir: Option<PathBuf>,
    new_asset_open: bool,
    new_asset_name: String,
    new_asset_kind: Option<AssetKind>,
    new_folder_open: bool,
    new_folder_name: String,
    jira_link_open: bool,
    jira_target: Option<PathBuf>,
    jira_value: String,
    properties_target: Option<PathBuf>,
    properties_environment: Option<String>,
    properties_openapi: String,
}

impl AssetsState {
    /// (Re)scan `root`. Cheap enough to call on open and on Refresh.
    pub fn load(&mut self, root: PathBuf) {
        let root_changed = self.root.as_ref() != Some(&root);
        match ProjectIndex::scan(&root) {
            Ok(index) => {
                self.index = Some(index);
                match project_tree(&root) {
                    Ok(tree) => {
                        self.tree = tree;
                        self.error = None;
                    }
                    Err(error) => {
                        self.tree.clear();
                        self.error = Some(error);
                    }
                }
            }
            Err(d) => {
                self.index = None;
                self.tree.clear();
                self.error = Some(d.message);
            }
        }
        if root_changed {
            self.collapsed.clear();
            self.collapsed.extend(
                self.tree
                    .iter()
                    .filter(|node| {
                        node.directory
                            && node.path.file_name().is_none_or(|name| name != "requests")
                    })
                    .map(|node| node.path.clone()),
            );
        }
        if self
            .selected_dir
            .as_ref()
            .is_none_or(|directory| !directory.starts_with(&root) || !directory.is_dir())
        {
            self.selected_dir = Some(root.join("requests"));
        }
        self.root = Some(root);
    }

    pub fn request_path(&self, id: &str) -> Option<PathBuf> {
        self.index
            .as_ref()?
            .requests
            .iter()
            .find(|request| request.id == id)
            .map(|request| PathBuf::from(&request.path))
    }

    pub fn is_loaded(&self) -> bool {
        self.index.is_some()
    }

    pub fn project_name(&self) -> Option<String> {
        self.root
            .as_ref()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    pub fn project_root(&self) -> Option<PathBuf> {
        self.root.clone()
    }

    pub fn selected_directory(&self) -> Option<PathBuf> {
        self.selected_dir.clone()
    }
}

/// Render the panel. Returns a ref string the user asked to copy (already
/// placed on the clipboard); the caller may also surface a status line.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    // Auto-load from an open v0 workspace root if it looks like a v1 project,
    // otherwise offer a folder picker.
    if state.assets.root.is_none() {
        if let Some(ws) = &state.workspace {
            if ws.root.join("project.json").exists() {
                state.assets.load(ws.root.clone());
            }
        }
    }
    // Track the active request's directory for relative-ref suggestions.
    state.assets.base_dir = active_request_dir(state);
    let active_file = state.dialogs.v1_editor.active_file().map(Path::to_path_buf);

    let root = state.assets.root.clone();
    if let Some(root) = root.as_deref() {
        state.git.refresh(root, false);
    }
    let mut new_request = false;
    let mut new_asset = false;
    let mut new_folder = false;
    let mut refresh = false;
    let mut new_sequence = false;
    let mut migrate_request = false;
    let mut migrate_tree = false;
    let mut switch_project = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(root.is_some(), |ui| {
            ui.menu_button(format!("{}  Request", icons::ADD), |ui| {
                if ui
                    .selectable_label(true, format!("{}  Request", icons::ADD))
                    .clicked()
                {
                    new_request = true;
                    ui.close();
                }
                if ui.button(format!("{}  New asset…", icons::ADD)).clicked() {
                    new_asset = true;
                    ui.close();
                }
                if ui.button(format!("{}  Add files…", icons::ADD)).clicked() {
                    if let Some(directory) = root
                        .as_deref()
                        .map(|root| request_directory(&state.assets, root))
                    {
                        add_files(state, &directory);
                    }
                    ui.close();
                }
            });
        });
        new_folder = ui
            .add_enabled(
                root.is_some(),
                egui::Button::new(format!("{}  Folder", icons::ADD)),
            )
            .on_hover_text("New folder")
            .clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            refresh = ui
                .add_enabled(root.is_some(), egui::Button::new("↻"))
                .on_hover_text("Refresh project index")
                .clicked();
            ui.menu_button(icons::ELLIPSIS, |ui| {
                if ui.button("New sequence…").clicked() {
                    new_sequence = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Migrate request…").clicked() {
                    migrate_request = true;
                    ui.close();
                }
                if ui.button("Migrate tree…").clicked() {
                    migrate_tree = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Switch project…").clicked() {
                    switch_project = true;
                    ui.close();
                }
            });
        });
    });

    if refresh {
        if let Some(root) = root.clone() {
            state.assets.load(root.clone());
            state.git.refresh(&root, true);
        }
    }
    if new_request {
        if let Some(root) = root.clone() {
            let directory = request_directory(&state.assets, &root);
            state
                .dialogs
                .v1_editor
                .open_new_in(root, directory, state.active_env.clone());
        }
    }
    if new_folder {
        state.assets.new_folder_open = true;
    }
    if new_asset {
        state.assets.new_asset_open = true;
        state
            .assets
            .new_asset_kind
            .get_or_insert(AssetKind::Assertion);
    }
    if let Some(root) = root.as_deref() {
        if new_sequence {
            create_sequence(state, root);
        }
        if migrate_request {
            migrate_legacy_request(state, root);
        }
        if migrate_tree {
            migrate_legacy_tree(state, root);
        }
    }
    if switch_project {
        open_standalone_project(state, bridge);
    }

    if let Some(err) = &state.assets.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
        return;
    }
    let Some(index) = &state.assets.index else {
        ui.add_space(8.0);
        ui.weak("Open a project to browse requests and reusable assets.");
        return;
    };

    ui.add_space(6.0);
    let selected = state
        .assets
        .selected_dir
        .as_deref()
        .map(|directory| forge_core::reqv1::index::relative_path(Path::new(&index.root), directory))
        .unwrap_or_else(|| ".".to_string());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (view, label) in [(ProjectView::Files, "Files"), (ProjectView::Git, "Git")] {
            let active = state.assets.view == view;
            let response = ui.add(
                egui::Label::new(RichText::new(label).color(if active {
                    ui.visuals().hyperlink_color
                } else {
                    ui.visuals().weak_text_color()
                }))
                .sense(egui::Sense::click()),
            );
            if active {
                ui.painter().line_segment(
                    [
                        egui::pos2(response.rect.left(), response.rect.bottom() + 4.0),
                        egui::pos2(response.rect.right(), response.rect.bottom() + 4.0),
                    ],
                    egui::Stroke::new(2.0, ui.visuals().hyperlink_color),
                );
            }
            if response.clicked() {
                state.assets.view = view;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let detail = match state.assets.view {
                ProjectView::Files => selected,
                ProjectView::Git => state
                    .git
                    .status
                    .as_ref()
                    .and_then(|status| status.branch.as_deref())
                    .map(|branch| format!("{} {branch}", icons::BRANCH))
                    .unwrap_or_else(|| "No repository".to_string()),
            };
            ui.label(RichText::new(detail).monospace().small().weak());
        });
    });
    ui.add_space(4.0);
    ui.separator();

    // Broken refs first — the thing an author most needs to see.
    if state.assets.view == ProjectView::Files && !index.broken.is_empty() {
        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            RichText::new(format!("{} broken ref(s)", index.broken.len()))
                .color(ui.visuals().error_fg_color),
        )
        .default_open(true)
        .show(ui, |ui| {
            for b in &index.broken {
                ui.label(
                    RichText::new(format!("{} {}", b.request, b.instance_path))
                        .monospace()
                        .small(),
                );
                ui.label(
                    RichText::new(format!("  {} — {}", b.reference, b.message))
                        .small()
                        .weak(),
                );
            }
        });
        ui.separator();
    }

    let base_dir = state.assets.base_dir.clone();
    let mut to_copy: Option<(String, String)> = None; // (ref, kind-of-thing)
    let mut to_open: Option<PathBuf> = None;
    let mut to_edit: Option<std::path::PathBuf> = None;
    let mut to_run: Option<Vec<PathBuf>> = None;
    let mut to_sequence: Option<PathBuf> = None;
    let mut to_jira: Option<PathBuf> = None;
    let mut project_action: Option<ProjectAction> = None;
    let mut git_add = None;
    let mut git_pending = None;
    let active_env = state.active_env.clone();
    let tree = vec![ProjectNode {
        path: PathBuf::from(&index.root),
        directory: true,
        children: state.assets.tree.clone(),
        ticket: forge_core::reqv1::effective_ticket(Path::new(&index.root), Path::new(&index.root))
            .ok()
            .flatten(),
    }];
    let project_view = state.assets.view;
    let git_status = state.git.status.clone();

    egui::ScrollArea::vertical()
        .id_salt("assets-sa-1")
        .auto_shrink([false, false])
        .show(ui, |ui| match project_view {
            ProjectView::Files if tree.is_empty() => {
                ui.add_space(12.0);
                ui.weak("This project is empty.");
            }
            ProjectView::Files => {
                project_nodes(
                    ui,
                    &tree,
                    Path::new(&index.root),
                    index,
                    active_file.as_deref(),
                    base_dir.as_deref(),
                    &mut state.assets.collapsed,
                    &mut state.assets.selected_dir,
                    &mut to_copy,
                    &mut to_open,
                    &mut to_edit,
                    &mut to_run,
                    &mut to_sequence,
                    &mut to_jira,
                    &mut project_action,
                    git_status.as_ref(),
                    &mut git_add,
                    &mut git_pending,
                );
            }
            ProjectView::Git => git_nodes(
                ui,
                git_status.as_ref(),
                Path::new(&index.root),
                &mut to_open,
                &mut to_edit,
            ),
        });

    if let Some(target) = to_jira {
        state.assets.jira_value = forge_core::reqv1::own_ticket(&target)
            .ok()
            .flatten()
            .unwrap_or_default();
        state.assets.jira_target = Some(target);
        state.assets.jira_link_open = true;
    }

    if let Some(file) = to_edit {
        if let Err(error) = state.dialogs.v1_editor.open_file(file, active_env.clone()) {
            state.status = Some(StatusMessage::error(error));
        }
    }
    if let Some(sequence_file) = to_sequence {
        let files = std::fs::read_to_string(&sequence_file)
            .map_err(|error| format!("cannot read {}: {error}", sequence_file.display()))
            .and_then(|text| {
                forge_core::reqv1::SequenceDocument::parse(&text)
                    .map_err(|error| format!("invalid sequence: {error}"))
            })
            .and_then(|sequence| sequence.resolve_files(std::path::Path::new(&index.root)));
        match files {
            Ok(files) => state.dialogs.v1_editor.run_sequence(
                PathBuf::from(&index.root),
                files,
                active_env.clone(),
                bridge,
            ),
            Err(error) => state.status = Some(StatusMessage::error(error)),
        }
    }
    if let Some(files) = to_run {
        state
            .dialogs
            .v1_editor
            .run_affected(PathBuf::from(&index.root), files, active_env, bridge);
    }

    if let Some(action) = project_action {
        handle_project_action(action, state);
    }
    if let Some(path) = git_add {
        let result = root
            .as_deref()
            .ok_or_else(|| "No project open".to_string())
            .and_then(|root| crate::git::add(root, &path));
        match result {
            Ok(()) => {
                if let Some(root) = root.as_deref() {
                    state.git.refresh(root, true);
                }
                state.status = Some(StatusMessage::info("Changes staged"));
            }
            Err(error) => state.status = Some(StatusMessage::error(error)),
        }
    }
    if let Some(pending) = git_pending {
        state.collections.pending = Some(pending);
        state.collections.pending_input.clear();
    }

    if let Some((r, what)) = to_copy {
        ui.ctx().copy_text(r.clone());
        state.status = Some(StatusMessage::info(format!("Copied {what}: {r}")));
    }
    if let Some(path) = to_open {
        let _ = open::that(path);
    }
    jira_link_dialog(ui.ctx(), state);
    new_folder_dialog(ui.ctx(), state);
    new_asset_dialog(ui.ctx(), state);
    properties_dialog(ui.ctx(), state);
    collections::show_modals(ui, state);
}

#[allow(clippy::too_many_arguments)]
fn project_nodes(
    ui: &mut Ui,
    nodes: &[ProjectNode],
    root: &Path,
    index: &ProjectIndex,
    active_file: Option<&Path>,
    base_dir: Option<&Path>,
    collapsed: &mut std::collections::HashSet<PathBuf>,
    selected_dir: &mut Option<PathBuf>,
    to_copy: &mut Option<(String, String)>,
    to_open: &mut Option<PathBuf>,
    to_edit: &mut Option<PathBuf>,
    to_run: &mut Option<Vec<PathBuf>>,
    to_sequence: &mut Option<PathBuf>,
    to_jira: &mut Option<PathBuf>,
    project_action: &mut Option<ProjectAction>,
    git_status: Option<&GitStatus>,
    git_add: &mut Option<PathBuf>,
    git_pending: &mut Option<PendingAction>,
) {
    for node in nodes {
        if node.directory {
            let open = !collapsed.contains(&node.path);
            let (response, ticket_response) = ui
                .horizontal(|ui| {
                    let triangle = if open {
                        icons::TRIANGLE_DOWN
                    } else {
                        icons::TRIANGLE_RIGHT
                    };
                    if ui.small_button(triangle).clicked() {
                        if open {
                            collapsed.insert(node.path.clone());
                        } else {
                            collapsed.remove(&node.path);
                        }
                    }
                    let name = node
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy())
                        .unwrap_or_default();
                    let folder_icon = match name.as_ref() {
                        "assets" => icons::ASSETS,
                        "environments" => icons::ENVIRONMENT,
                        _ => icons::FOLDER,
                    };
                    let response = ui
                        .selectable_label(
                            selected_dir.as_ref() == Some(&node.path),
                            RichText::new(format!("{folder_icon}  {name}")).size(15.0),
                        )
                        .on_hover_text(node.path.display().to_string());
                    if response.clicked() {
                        *selected_dir = Some(node.path.clone());
                    }
                    let ticket_response = ticket_icon(ui, node, root);
                    (response, ticket_response)
                })
                .inner;
            let response =
                ticket_response.map_or(response.clone(), |ticket| response.union(ticket));
            folder_context_menu(
                response,
                node,
                root,
                to_run,
                to_jira,
                project_action,
                git_status,
                git_add,
                git_pending,
            );
            if open {
                ui.indent(&node.path, |ui| {
                    project_nodes(
                        ui,
                        &node.children,
                        root,
                        index,
                        active_file,
                        base_dir,
                        collapsed,
                        selected_dir,
                        to_copy,
                        to_open,
                        to_edit,
                        to_run,
                        to_sequence,
                        to_jira,
                        project_action,
                        git_status,
                        git_add,
                        git_pending,
                    );
                });
            }
            continue;
        }

        let rel_path = forge_core::reqv1::index::relative_path(root, &node.path);
        let name = node
            .path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let icon = if rel_path.ends_with(".request.json") {
            icons::REQUEST
        } else if rel_path.ends_with(".sequence.json") {
            icons::PLAY
        } else {
            icons::CODE
        };
        let (response, ticket_response) = ui
            .horizontal(|ui| {
                let response = ui
                    .selectable_label(
                        active_file == Some(node.path.as_path()),
                        RichText::new(format!("{icon}  {name}")).size(14.0),
                    )
                    .on_hover_text(&rel_path);
                let ticket_response = rel_path
                    .ends_with(".request.json")
                    .then(|| ticket_icon(ui, node, root))
                    .flatten();
                (response, ticket_response)
            })
            .inner;
        let clicked = response.clicked();
        let response = ticket_response.map_or(response.clone(), |ticket| response.union(ticket));
        if clicked {
            if rel_path.ends_with(".request.json") {
                *to_edit = Some(node.path.clone());
            } else {
                *to_open = Some(node.path.clone());
            }
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                if rel_path.ends_with(".request.json") {
                    *to_edit = Some(node.path.clone());
                } else {
                    *to_open = Some(node.path.clone());
                }
                ui.close();
            }
            if let Some(sequence) = index
                .sequences
                .iter()
                .find(|sequence| sequence.rel_path == rel_path)
            {
                if ui.button("Run sequence").clicked() {
                    *to_sequence = Some(PathBuf::from(&sequence.path));
                    ui.close();
                }
            }
            if let Some(asset) = index.assets.iter().find(|asset| asset.rel_path == rel_path) {
                let base = base_dir.unwrap_or(root);
                let reference = index.suggest_ref(asset, base);
                if ui.button("Copy reference").clicked() {
                    *to_copy = Some((reference, "reference".to_string()));
                    ui.close();
                }
                if !asset.used_by.is_empty() && ui.button("Run affected requests").clicked() {
                    *to_run = Some(
                        asset
                            .used_by
                            .iter()
                            .map(|usage| root.join(&usage.request))
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect(),
                    );
                    ui.close();
                }
            }
            if rel_path.ends_with(".request.json") {
                ui.separator();
                export_menu(ui, &node.path, project_action);
                if ui.button("Properties…").clicked() {
                    *project_action = Some(ProjectAction::Properties(node.path.clone()));
                    ui.close();
                }
                ticket_menu(ui, node, to_jira);
            }
            asset_git_menu(
                ui,
                git_status,
                &node.path,
                name.as_ref(),
                git_add,
                git_pending,
            );
        });
    }
}

fn git_nodes(
    ui: &mut Ui,
    status: Option<&GitStatus>,
    root: &Path,
    to_open: &mut Option<PathBuf>,
    to_edit: &mut Option<PathBuf>,
) {
    let Some(status) = status else {
        ui.add_space(12.0);
        ui.weak("This project is not in a Git repository.");
        return;
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut files = status
        .files
        .iter()
        .filter(|(path, _)| path.starts_with(&root))
        .map(|(path, status)| (path.clone(), *status))
        .collect::<Vec<_>>();
    files.sort_by(|(path_a, status_a), (path_b, status_b)| {
        git_presentation(*status_a)
            .0
            .cmp(&git_presentation(*status_b).0)
            .then_with(|| path_a.cmp(path_b))
    });
    if files.is_empty() {
        ui.add_space(12.0);
        ui.weak("Working tree clean.");
        return;
    }

    let mut current_group = None;
    for (path, status) in files {
        let (order, group, code, color) = git_presentation(status);
        if current_group != Some(order) {
            if current_group.is_some() {
                ui.add_space(8.0);
            }
            ui.label(RichText::new(group).strong());
            current_group = Some(order);
        }
        let relative = forge_core::reqv1::index::relative_path(&root, &path);
        let response = ui
            .horizontal(|ui| {
                ui.label(RichText::new(code).monospace().color(color));
                ui.selectable_label(false, RichText::new(&relative).monospace().small())
                    .on_hover_text(path.display().to_string())
            })
            .inner;
        if response.clicked() {
            if relative.ends_with(".request.json") {
                *to_edit = Some(path.clone());
            } else {
                *to_open = Some(path.clone());
            }
        }
        response.context_menu(|ui| {
            if ui.button("Open").clicked() {
                if relative.ends_with(".request.json") {
                    *to_edit = Some(path.clone());
                } else {
                    *to_open = Some(path.clone());
                }
                ui.close();
            }
        });
    }
}

fn git_presentation(status: FileStatus) -> (u8, &'static str, &'static str, egui::Color32) {
    match status {
        FileStatus::Conflicted => (
            0,
            "Conflicts",
            "!",
            egui::Color32::from_rgb(0xDB, 0x5C, 0x5C),
        ),
        FileStatus::Untracked => (
            1,
            "Untracked Files",
            "?",
            egui::Color32::from_rgb(0xD9, 0xA3, 0x43),
        ),
        FileStatus::Modified => (2, "Changes", "M", egui::Color32::from_rgb(0x4A, 0x90, 0xD9)),
        FileStatus::StagedModified => (
            3,
            "Partially Staged",
            "MM",
            egui::Color32::from_rgb(0x4A, 0x90, 0xD9),
        ),
        FileStatus::Added => (
            4,
            "Staged Changes",
            "A",
            egui::Color32::from_rgb(0x59, 0xA8, 0x69),
        ),
        FileStatus::Staged => (
            4,
            "Staged Changes",
            "M",
            egui::Color32::from_rgb(0x59, 0xA8, 0x69),
        ),
    }
}

fn ticket_icon(ui: &mut Ui, node: &ProjectNode, root: &Path) -> Option<egui::Response> {
    const MINIMUM_SPACE: f32 = 28.0;
    let ticket = node.ticket.as_ref()?;
    if ui.available_width() < MINIMUM_SPACE {
        return None;
    }
    let inherited = ticket.source != node.path;
    let color = if inherited {
        ui.visuals().weak_text_color()
    } else {
        ui.visuals().hyperlink_color
    };
    let source = forge_core::reqv1::index::relative_path(root, &ticket.source);
    Some(
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(RichText::new(icons::JIRA).small().color(color))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(if inherited {
                format!(
                    "Jira {} · inherited from {source}",
                    forge_core::reqv1::ticket_label(&ticket.value)
                )
            } else {
                format!(
                    "Jira {} · linked here",
                    forge_core::reqv1::ticket_label(&ticket.value)
                )
            })
        })
        .inner,
    )
}

#[allow(clippy::too_many_arguments)]
fn folder_context_menu(
    response: egui::Response,
    node: &ProjectNode,
    root: &Path,
    to_run: &mut Option<Vec<PathBuf>>,
    to_jira: &mut Option<PathBuf>,
    action: &mut Option<ProjectAction>,
    git_status: Option<&GitStatus>,
    git_add: &mut Option<PathBuf>,
    git_pending: &mut Option<PendingAction>,
) {
    response.context_menu(|ui| {
        let mut requests = Vec::new();
        collect_request_paths(node, &mut requests);
        let run_label = if node.path == root {
            "Run project"
        } else {
            "Run folder"
        };
        if ui
            .add_enabled(!requests.is_empty(), egui::Button::new(run_label))
            .clicked()
        {
            *to_run = Some(requests);
            ui.close();
        }
        ui.separator();
        if (node.path == root || node.path.starts_with(root.join("requests")))
            && ui.button("New request…").clicked()
        {
            *action = Some(ProjectAction::NewRequest(node.path.clone()));
            ui.close();
        }
        if ui.button("New folder…").clicked() {
            *action = Some(ProjectAction::NewFolder(node.path.clone()));
            ui.close();
        }
        if ui.button("Add files…").clicked() {
            *action = Some(ProjectAction::AddFiles(node.path.clone()));
            ui.close();
        }
        if ui.button("Beautify JSON recursively").clicked() {
            *action = Some(ProjectAction::Beautify(node.path.clone()));
            ui.close();
        }
        export_menu(ui, &node.path, action);
        if ui.button("Import Forge bundle…").clicked() {
            *action = Some(ProjectAction::Import(node.path.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Properties…").clicked() {
            *action = Some(ProjectAction::Properties(node.path.clone()));
            ui.close();
        }
        if ui.button("Open in file manager").clicked() {
            *action = Some(ProjectAction::Open(node.path.clone()));
            ui.close();
        }
        ui.separator();
        ticket_menu(ui, node, to_jira);
        asset_git_menu(
            ui,
            git_status,
            &node.path,
            node.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project"),
            git_add,
            git_pending,
        );
    });
}

fn export_menu(ui: &mut Ui, source: &Path, action: &mut Option<ProjectAction>) {
    ui.menu_button("Export", |ui| {
        if ui.button("Forge JSON bundle…").clicked() {
            *action = Some(ProjectAction::Export(
                source.to_path_buf(),
                BundleFormat::Json,
            ));
            ui.close();
        }
        if ui.button("cURL script…").clicked() {
            *action = Some(ProjectAction::Export(
                source.to_path_buf(),
                BundleFormat::Curl,
            ));
            ui.close();
        }
    });
}

fn asset_git_menu(
    ui: &mut Ui,
    git_status: Option<&GitStatus>,
    path: &Path,
    name: &str,
    git_add: &mut Option<PathBuf>,
    git_pending: &mut Option<PendingAction>,
) {
    use FileStatus as F;
    let Some(status) = git_status.and_then(|status| status.of(path)) else {
        return;
    };
    ui.separator();
    ui.menu_button("Git", |ui| {
        if matches!(
            status,
            F::Untracked | F::Modified | F::StagedModified | F::Conflicted
        ) && ui.button("Stage").clicked()
        {
            *git_add = Some(path.to_path_buf());
            ui.close();
        }
        if matches!(
            status,
            F::Modified | F::Added | F::Staged | F::StagedModified | F::Conflicted
        ) && ui.button("Revert changes…").clicked()
        {
            *git_pending = Some(PendingAction::GitRevert(
                path.to_path_buf(),
                name.to_string(),
            ));
            ui.close();
        }
        if ui.button("Commit…").clicked() {
            *git_pending = Some(PendingAction::GitCommit(
                path.to_path_buf(),
                name.to_string(),
            ));
            ui.close();
        }
    });
}

fn collect_request_paths(node: &ProjectNode, requests: &mut Vec<PathBuf>) {
    if node.directory {
        for child in &node.children {
            collect_request_paths(child, requests);
        }
    } else if node.path.to_string_lossy().ends_with(".request.json") {
        requests.push(node.path.clone());
    }
}

fn ticket_menu(ui: &mut Ui, node: &ProjectNode, to_jira: &mut Option<PathBuf>) {
    if let Some(ticket) = &node.ticket {
        if let Some(url) = jira_url(&ticket.value) {
            if ui.button("Open Jira ticket").clicked() {
                let _ = open::that(url);
                ui.close();
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Open Jira ticket"))
                .on_disabled_hover_text("Link the full Jira URL to open it directly");
        }
    }
    let own = node
        .ticket
        .as_ref()
        .is_some_and(|ticket| ticket.source == node.path);
    let action = if own {
        "Edit Jira ticket…"
    } else if node.ticket.is_some() {
        "Override Jira ticket…"
    } else {
        "Link Jira ticket…"
    };
    if ui.button(action).clicked() {
        *to_jira = Some(node.path.clone());
        ui.close();
    }
    if let Some(ticket) = &node.ticket {
        if ui.button("Copy Jira ticket").clicked() {
            ui.ctx().copy_text(ticket.value.clone());
            ui.close();
        }
    }
}

fn jira_url(value: &str) -> Option<&str> {
    let value = value.trim();
    (value.starts_with("https://") || value.starts_with("http://")).then_some(value)
}

fn project_tree(root: &Path) -> Result<Vec<ProjectNode>, String> {
    fn read(root: &Path, directory: &Path) -> Result<Vec<ProjectNode>, String> {
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        let mut nodes = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
            let path = entry.path();
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(".git" | ".forge-local" | "node_modules" | "target")
            ) || name
                .to_string_lossy()
                .ends_with(forge_core::reqv1::tickets::FILE_TICKET_SUFFIX)
                || name
                    .to_string_lossy()
                    .ends_with(forge_core::reqv1::environment_scope::FILE_ENVIRONMENT_SUFFIX)
                || name
                    .to_string_lossy()
                    .ends_with(forge_core::reqv1::openapi_scope::FILE_OPENAPI_SUFFIX)
                || name.to_string_lossy().ends_with(".assertions.json")
                || name.to_string_lossy().ends_with(".hooks.json")
            {
                continue;
            }
            let directory = entry
                .file_type()
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
                .is_dir();
            let children = if directory {
                read(root, &path)?
            } else {
                Vec::new()
            };
            let ticket = forge_core::reqv1::effective_ticket(root, &path)?;
            nodes.push(ProjectNode {
                path,
                directory,
                children,
                ticket,
            });
        }
        nodes.sort_by_key(|node| {
            let name = node
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let priority = match name.as_str() {
                "requests" => 0,
                "sequences" => 1,
                "assets" => 2,
                "environments" => 3,
                _ if node.directory => 4,
                "project.json" => 5,
                _ => 6,
            };
            (priority, name)
        });
        Ok(nodes)
    }

    read(root, root)
}

fn request_directory(assets: &AssetsState, root: &Path) -> PathBuf {
    let requests = root.join("requests");
    assets
        .selected_dir
        .as_ref()
        .filter(|directory| directory.starts_with(&requests) && directory.is_dir())
        .cloned()
        .unwrap_or(requests)
}

fn folder_parent(assets: &AssetsState) -> Option<PathBuf> {
    let root = assets.root.as_ref()?;
    assets
        .selected_dir
        .as_ref()
        .filter(|directory| directory.starts_with(root) && directory.is_dir())
        .cloned()
        .or_else(|| Some(root.join("requests")))
}

fn valid_folder_name(name: &str) -> bool {
    let mut components = Path::new(name.trim()).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn handle_project_action(action: ProjectAction, state: &mut AppState) {
    match action {
        ProjectAction::NewRequest(directory) => {
            let Some(root) = state.assets.root.clone() else {
                return;
            };
            let directory = if directory == root {
                root.join("requests")
            } else {
                directory
            };
            state
                .dialogs
                .v1_editor
                .open_new_in(root, directory, state.active_env.clone());
        }
        ProjectAction::NewFolder(directory) => {
            state.assets.selected_dir = Some(directory);
            state.assets.new_folder_open = true;
        }
        ProjectAction::AddFiles(directory) => add_files(state, &directory),
        ProjectAction::Beautify(directory) => beautify_folder(state, &directory),
        ProjectAction::Export(source, format) => export_path(state, &source, format),
        ProjectAction::Import(destination) => import_into(state, &destination),
        ProjectAction::Properties(target) => {
            state.assets.properties_environment =
                forge_core::reqv1::own_environment(&target).ok().flatten();
            state.assets.properties_openapi = forge_core::reqv1::own_openapi(&target)
                .ok()
                .flatten()
                .unwrap_or_default();
            state.assets.properties_target = Some(target);
        }
        ProjectAction::Open(directory) => {
            if let Err(error) = open::that(&directory) {
                state.status = Some(StatusMessage::error(format!(
                    "cannot open {}: {error}",
                    directory.display()
                )));
            }
        }
    }
}

fn export_path(state: &mut AppState, source: &Path, format: BundleFormat) {
    let Some(root) = state.assets.root.clone() else {
        return;
    };
    if state.dialogs.v1_editor.has_unsaved_request_under(source) {
        state.status = Some(StatusMessage::error(
            "Save the open request before exporting this scope",
        ));
        return;
    }
    let base = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export")
        .strip_suffix(".request.json")
        .unwrap_or_else(|| {
            source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("export")
        });
    let file_name = format!("{base}.{}", format.extension());
    let dialog = rfd::FileDialog::new()
        .set_directory(source.parent().unwrap_or(&root))
        .set_file_name(file_name);
    let output = match format {
        BundleFormat::Json => dialog
            .add_filter("Forge JSON bundle", &["json"])
            .save_file(),
        BundleFormat::Curl => dialog.add_filter("Forge cURL script", &["sh"]).save_file(),
    };
    let Some(output) = output else {
        return;
    };
    match forge_core::reqv1::export_bundle(&root, source, format, &output) {
        Ok(summary) => {
            state.status = Some(StatusMessage::info(format!(
                "Exported {} request(s) and {} file(s) to {}",
                summary.requests,
                summary.files,
                summary.output.display()
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
}

fn import_into(state: &mut AppState, destination: &Path) {
    let Some(input) = rfd::FileDialog::new()
        .set_directory(destination)
        .add_filter("Forge bundle", &["json", "sh"])
        .pick_file()
    else {
        return;
    };
    match forge_core::reqv1::import_bundle(&input, destination) {
        Ok(summary) => {
            if let Some(root) = state.assets.root.clone() {
                state.assets.load(root.clone());
                state.git.refresh(&root, true);
            }
            state.status = Some(StatusMessage::info(format!(
                "Imported {} file(s) below {}",
                summary.files.len(),
                destination.display()
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
}

fn add_files(state: &mut AppState, directory: &Path) {
    let Some(files) = rfd::FileDialog::new().set_directory(directory).pick_files() else {
        return;
    };
    match copy_files_to_directory(&files, directory) {
        Ok(copied) => {
            if let Some(root) = state.assets.root.clone() {
                state.assets.load(root.clone());
                state.git.refresh(&root, true);
            }
            state.status = Some(StatusMessage::info(format!(
                "Added {copied} file(s) to {}",
                directory.display()
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
}

fn copy_files_to_directory(files: &[PathBuf], directory: &Path) -> Result<usize, String> {
    let mut targets = std::collections::BTreeSet::new();
    let copies = files
        .iter()
        .map(|source| {
            let name = source
                .file_name()
                .ok_or_else(|| format!("{} has no file name", source.display()))?;
            let target = directory.join(name);
            if source == &target {
                return Err(format!("{} is already in this folder", source.display()));
            }
            if target.exists() || !targets.insert(target.clone()) {
                return Err(format!("{} already exists", target.display()));
            }
            Ok((source, target))
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (source, target) in &copies {
        std::fs::copy(source, target).map_err(|error| {
            format!(
                "cannot copy {} to {}: {error}",
                source.display(),
                target.display()
            )
        })?;
    }
    Ok(copies.len())
}

fn beautify_folder(state: &mut AppState, directory: &Path) {
    if state.dialogs.v1_editor.has_unsaved_request_under(directory) {
        state.status = Some(StatusMessage::error(
            "Save or discard the open request before beautifying this folder",
        ));
        return;
    }
    match beautify_json_tree(directory) {
        Ok(changed) => {
            if let Err(error) = state
                .dialogs
                .v1_editor
                .reload_clean_request_under(directory)
            {
                state.status = Some(StatusMessage::error(error));
                return;
            }
            if let Some(root) = state.assets.root.clone() {
                state.assets.load(root.clone());
                state.git.refresh(&root, true);
            }
            state.status = Some(StatusMessage::info(format!(
                "Beautified {changed} JSON file(s) below {}",
                directory.display()
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
}

fn beautify_json_tree(directory: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    collect_json_files(directory, &mut files)?;
    let mut writes = Vec::new();
    for file in files {
        let current = std::fs::read_to_string(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let value: serde_json::Value = serde_json::from_str(&current)
            .map_err(|error| format!("invalid JSON in {}: {error}", file.display()))?;
        let mut formatted = serde_json::to_string_pretty(&value)
            .map_err(|error| format!("cannot format {}: {error}", file.display()))?;
        formatted.push('\n');
        if formatted != current {
            writes.push((file, formatted));
        }
    }
    for (file, formatted) in &writes {
        std::fs::write(file, formatted)
            .map_err(|error| format!("cannot write {}: {error}", file.display()))?;
    }
    Ok(writes.len())
}

fn collect_json_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some(".git" | ".forge-local" | "node_modules" | "target")
            ) {
                collect_json_files(&path, files)?;
            }
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}

fn properties_dialog(ctx: &egui::Context, state: &mut AppState) {
    let (Some(root), Some(target)) = (
        state.assets.root.clone(),
        state.assets.properties_target.clone(),
    ) else {
        return;
    };
    let request_count = state
        .assets
        .index
        .as_ref()
        .map(|index| {
            index
                .requests
                .iter()
                .filter(|request| Path::new(&request.path).starts_with(&target))
                .count()
        })
        .unwrap_or_default();
    let relative = forge_core::reqv1::index::relative_path(&root, &target);
    let kind = if target == root {
        "Project"
    } else if target.is_dir() {
        "Folder"
    } else {
        "Request"
    };
    let mut environments = state
        .assets
        .index
        .as_ref()
        .map(|index| index.environments.clone())
        .unwrap_or_default();
    environments.sort();
    let inherited = forge_core::reqv1::effective_environment(&root, &target)
        .ok()
        .flatten()
        .filter(|selection| selection.source != target);
    let previous_environment = state.assets.properties_environment.clone();
    let mut selected_environment = previous_environment.clone();
    let mut open = true;
    let mut close = false;
    let mut open_folder = false;
    let mut save_openapi = false;
    egui::Window::new(format!("{kind} properties"))
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::Grid::new("folder-properties")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.strong(
                        target
                            .file_name()
                            .map(|name| name.to_string_lossy())
                            .unwrap_or_default(),
                    );
                    ui.end_row();
                    ui.label("Project path");
                    ui.monospace(relative);
                    ui.end_row();
                    if target.is_dir() {
                        ui.label("Requests");
                        ui.label(request_count.to_string());
                        ui.end_row();
                    }
                    ui.label("Location");
                    ui.monospace(target.display().to_string());
                    ui.end_row();
                });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.strong("Environment");
            ui.add_space(4.0);
            let inherited_label = inherited
                .as_ref()
                .map(|selection| format!("Inherit ({})", selection.value))
                .unwrap_or_else(|| "Inherit (none)".to_string());
            egui::ComboBox::from_id_salt("properties-environment")
                .selected_text(
                    selected_environment
                        .clone()
                        .unwrap_or_else(|| inherited_label.clone()),
                )
                .width(260.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut selected_environment, None, inherited_label);
                    for environment in &environments {
                        ui.selectable_value(
                            &mut selected_environment,
                            Some(environment.clone()),
                            environment,
                        );
                    }
                });
            if let Some(selection) = &inherited {
                let source = forge_core::reqv1::index::relative_path(&root, &selection.source);
                ui.weak(format!(
                    "Inherited from {source}. Requests can override this value."
                ));
            } else if selected_environment.is_some() {
                ui.weak("Applies below this node unless a child overrides it.");
            } else if environments.is_empty() {
                ui.weak("No environments exist yet. Create one under environments/.");
            } else {
                ui.weak("No default. Requests run with an empty environment.");
            }
            if selected_environment
                .as_ref()
                .is_some_and(|selected| !environments.contains(selected))
            {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "This environment no longer exists in the project.",
                );
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.strong("OpenAPI source")
                .on_hover_text("Requests below this folder inherit this URL or project-relative specification path.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.assets.properties_openapi)
                        .hint_text("https://api.example.com/openapi.json")
                        .desired_width(330.0),
                );
                save_openapi = ui.button("Save").clicked();
            });
            if state.assets.properties_openapi.trim().is_empty() {
                if let Ok(Some(selection)) =
                    forge_core::reqv1::effective_openapi(&root, &target)
                {
                    if selection.source != target {
                        let source =
                            forge_core::reqv1::index::relative_path(&root, &selection.source);
                        ui.label(format!("Inherited from {source}"))
                            .on_hover_text(selection.value);
                    }
                }
            }
            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                open_folder = ui.button("Reveal in file manager").clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button("Close").clicked();
                });
            });
        });
    if selected_environment != previous_environment {
        let result = match selected_environment.as_deref() {
            Some(environment) if environments.iter().any(|item| item == environment) => {
                forge_core::reqv1::set_environment(&target, environment)
            }
            Some(environment) => Err(format!(
                "environment {environment} does not exist in this project"
            )),
            None => forge_core::reqv1::remove_environment(&target),
        };
        match result {
            Ok(()) => {
                state.assets.properties_environment = selected_environment.clone();
                state.git.refresh(&root, true);
                let label = selected_environment
                    .as_deref()
                    .map(|environment| format!("Environment set to {environment}"))
                    .unwrap_or_else(|| "Environment now inherits from its parent".to_string());
                state.status = Some(StatusMessage::info(label));
            }
            Err(error) => {
                state.assets.properties_environment = previous_environment;
                state.status = Some(StatusMessage::error(error));
            }
        }
    }
    if save_openapi {
        let value = state.assets.properties_openapi.trim().to_string();
        let result = if value.is_empty() {
            forge_core::reqv1::remove_openapi(&target)
        } else {
            forge_core::reqv1::set_openapi(&target, &value)
        };
        match result {
            Ok(()) => {
                state.openapi = None;
                state.openapi_source = None;
                state.git.refresh(&root, true);
                state.status = Some(StatusMessage::info(if value.is_empty() {
                    "OpenAPI source now inherits from its parent".to_string()
                } else {
                    "OpenAPI source saved".to_string()
                }));
            }
            Err(error) => state.status = Some(StatusMessage::error(error)),
        }
    }
    if open_folder {
        let location = if target.is_dir() {
            target.as_path()
        } else {
            target.parent().unwrap_or(&root)
        };
        let _ = open::that(location);
    }
    if open && !close {
        state.assets.properties_target = Some(target);
    } else {
        state.assets.properties_target = None;
        state.assets.properties_environment = None;
        state.assets.properties_openapi.clear();
    }
}

fn jira_link_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.assets.jira_link_open {
        return;
    }
    let (Some(root), Some(target)) = (state.assets.root.clone(), state.assets.jira_target.clone())
    else {
        state.assets.jira_link_open = false;
        return;
    };
    let has_own = forge_core::reqv1::own_ticket(&target)
        .ok()
        .flatten()
        .is_some();
    let inherited = (!has_own)
        .then(|| {
            forge_core::reqv1::effective_ticket(&root, &target)
                .ok()
                .flatten()
        })
        .flatten();
    let mut open = true;
    let mut save = false;
    let mut remove = false;
    let mut cancel = false;
    let accent = state.theme.accent_color();
    egui::Window::new("Jira ticket")
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let relative = forge_core::reqv1::index::relative_path(&root, &target);
            ui.label(RichText::new(relative).monospace().strong());
            ui.label("Paste the full Jira ticket URL.");
            ui.add_space(8.0);
            ui.add_sized(
                [ui.available_width(), 30.0],
                egui::TextEdit::singleline(&mut state.assets.jira_value)
                    .hint_text("https://jira.example/browse/API-123"),
            );
            if !state.assets.jira_value.trim().is_empty()
                && jira_url(&state.assets.jira_value).is_none()
            {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "A full URL is required so the ticket can open without Jira configuration.",
                );
            }
            if let Some(ticket) = inherited {
                let source = forge_core::reqv1::index::relative_path(&root, &ticket.source);
                ui.label(
                    RichText::new(format!("Inherited: {} from {source}", ticket.value))
                        .small()
                        .weak(),
                );
            } else {
                ui.label(
                    RichText::new("Child folders and tests inherit this link automatically.")
                        .small()
                        .weak(),
                );
            }
            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if has_own {
                    remove = ui.button("Remove own link").clicked();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save = ui
                        .add_enabled(
                            jira_url(&state.assets.jira_value).is_some(),
                            crate::theme::primary_button("Save link", accent),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });
    state.assets.jira_link_open = open && !save && !remove && !cancel;
    if !(save || remove) {
        return;
    }

    let result = if save {
        forge_core::reqv1::set_ticket(&target, &state.assets.jira_value)
    } else {
        forge_core::reqv1::remove_ticket(&target)
    };
    match result {
        Ok(()) => {
            state.assets.jira_value.clear();
            state.assets.jira_target = None;
            state.assets.load(root);
            state.status = Some(StatusMessage::info(if save {
                "Jira ticket linked"
            } else {
                "Jira override removed"
            }));
        }
        Err(error) => {
            state.assets.jira_link_open = true;
            state.status = Some(StatusMessage::error(error));
        }
    }
}

fn new_folder_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.assets.new_folder_open {
        return;
    }
    let Some(parent) = folder_parent(&state.assets) else {
        state.assets.new_folder_open = false;
        return;
    };
    let mut open = true;
    let mut create = false;
    let mut cancel = false;
    let accent = state.theme.accent_color();
    egui::Window::new("New folder")
        .collapsible(false)
        .resizable(false)
        .default_width(400.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Group related requests by story or feature.");
            ui.add_space(8.0);
            ui.label("Folder name");
            ui.add_sized(
                [ui.available_width(), 30.0],
                egui::TextEdit::singleline(&mut state.assets.new_folder_name).hint_text("checkout"),
            );
            let target = parent.join(state.assets.new_folder_name.trim());
            let display = state
                .assets
                .root
                .as_deref()
                .map(|root| forge_core::reqv1::index::relative_path(root, &target))
                .unwrap_or_else(|| target.display().to_string());
            ui.label(RichText::new(display).monospace().small().weak());
            ui.add_space(10.0);
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                create = ui
                    .add_enabled(
                        valid_folder_name(&state.assets.new_folder_name),
                        crate::theme::primary_button("Create folder", accent),
                    )
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    state.assets.new_folder_open = open && !create && !cancel;
    if !create {
        return;
    }

    let target = parent.join(state.assets.new_folder_name.trim());
    if let Err(error) = std::fs::create_dir_all(&parent).and_then(|_| std::fs::create_dir(&target))
    {
        state.assets.new_folder_open = true;
        state.status = Some(StatusMessage::error(format!(
            "cannot create {}: {error}",
            target.display()
        )));
        return;
    }
    state.assets.new_folder_name.clear();
    state.assets.selected_dir = Some(target.clone());
    state.assets.collapsed.remove(&parent);
    if let Some(root) = state.assets.root.clone() {
        state.assets.load(root);
    }
    state.status = Some(StatusMessage::info(format!(
        "Created folder {}",
        target.display()
    )));
}

fn open_standalone_project(state: &mut AppState, bridge: &Bridge) {
    let Some(dir) = rfd::FileDialog::new().pick_folder() else {
        return;
    };
    if !dir.join("project.json").is_file() {
        state.status = Some(StatusMessage::error(format!(
            "{} does not contain project.json",
            dir.display()
        )));
        return;
    }
    state.workspace = None;
    state.tabs.clear();
    state.active_tab = None;
    state.assets.load(dir.clone());
    match crate::panels::history::open_store(&dir) {
        Ok(store) => {
            state.history_store = Some(store);
            state.history_ui.loaded = false;
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
    if let Err(error) = bridge.send(crate::bridge::Cmd::LoadCookies {
        path: crate::panels::cookies::cookies_path(&dir),
    }) {
        state.status = Some(StatusMessage::error(error));
    }
    state.show_assets = true;
    state.dialogs.v1_editor.open_new(dir, None);
}

fn new_asset_dialog(ctx: &egui::Context, state: &mut AppState) {
    if !state.assets.new_asset_open {
        return;
    }
    let mut open = true;
    let mut create = false;
    let mut cancel = false;
    let accent = state.theme.accent_color();
    egui::Window::new("Create catalog asset")
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Create reusable behavior with catalog metadata.");
            ui.add_space(8.0);
            egui::Grid::new("new-asset-fields")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add_sized(
                        [280.0, 30.0],
                        egui::TextEdit::singleline(&mut state.assets.new_asset_name)
                            .hint_text("response-status"),
                    );
                    ui.end_row();
                    ui.label("Type");
                    egui::ComboBox::from_id_salt("new-asset-kind")
                        .width(280.0)
                        .selected_text(
                            state
                                .assets
                                .new_asset_kind
                                .map(AssetKind::label)
                                .unwrap_or("Choose type"),
                        )
                        .show_ui(ui, |ui| {
                            for kind in [
                                AssetKind::Assertion,
                                AssetKind::Hook,
                                AssetKind::Extractor,
                                AssetKind::Generator,
                                AssetKind::Mock,
                            ] {
                                ui.selectable_value(
                                    &mut state.assets.new_asset_kind,
                                    Some(kind),
                                    kind.label(),
                                );
                            }
                        });
                    ui.end_row();
                });
            if let Some(kind) = state.assets.new_asset_kind {
                let name = state.assets.new_asset_name.trim();
                ui.label(
                    RichText::new(format!(
                        "assets/{}/{}.js",
                        kind.label(),
                        if name.is_empty() { "name" } else { name }
                    ))
                    .monospace()
                    .small()
                    .weak(),
                );
            }
            ui.add_space(10.0);
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                create = ui
                    .add_enabled(
                        !state.assets.new_asset_name.trim().is_empty()
                            && state.assets.new_asset_kind.is_some(),
                        crate::theme::primary_button("Create asset", accent),
                    )
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    state.assets.new_asset_open = open && !create && !cancel;
    if !create {
        return;
    }
    let (Some(root), Some(kind)) = (state.assets.root.clone(), state.assets.new_asset_kind) else {
        return;
    };
    match forge_core::reqv1::scaffold_asset(&root, kind, state.assets.new_asset_name.trim()) {
        Ok(created) => {
            state.assets.new_asset_name.clear();
            state.assets.load(root);
            state.status = Some(StatusMessage::info(format!(
                "Created {}",
                created.code.display()
            )));
            let _ = open::that(created.code);
        }
        Err(error) => {
            state.assets.new_asset_open = true;
            state.status = Some(StatusMessage::error(error));
        }
    }
}

fn create_sequence(state: &mut AppState, root: &std::path::Path) {
    let Some(requests) = rfd::FileDialog::new()
        .set_directory(root.join("requests"))
        .add_filter("request", &["json"])
        .pick_files()
    else {
        return;
    };
    if requests.is_empty() {
        return;
    }
    let request_paths = requests
        .iter()
        .map(|request| forge_core::reqv1::index::relative_path(root, request))
        .collect::<Vec<_>>();
    if request_paths
        .iter()
        .any(|request| request.starts_with("../") || !request.ends_with(".request.json"))
    {
        state.status = Some(StatusMessage::error(
            "sequence requests must be *.request.json files inside the project",
        ));
        return;
    }
    let target =
        forge_core::reqv1::available_path(&root.join("sequences"), "sequence", ".sequence.json");
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("sequence")
        .strip_suffix(".sequence")
        .unwrap_or("sequence");
    let document = serde_json::json!({
        "formatVersion": 1,
        "kind": "sequence",
        "meta": {"id": stem, "name": stem.replace(['-', '_'], " ")},
        "requests": request_paths,
    });
    let mut json = serde_json::to_string_pretty(&document).unwrap_or_default();
    json.push('\n');
    if let Some(parent) = target.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            state.status = Some(StatusMessage::error(format!(
                "cannot create {}: {error}",
                parent.display()
            )));
            return;
        }
    }
    match std::fs::write(&target, json) {
        Ok(()) => {
            state.assets.load(root.to_path_buf());
            state.status = Some(StatusMessage::info(format!("Created {}", target.display())));
            let _ = open::that(target);
        }
        Err(error) => {
            state.status = Some(StatusMessage::error(format!(
                "cannot write {}: {error}",
                target.display()
            )));
        }
    }
}

fn migrate_legacy_request(state: &mut AppState, root: &std::path::Path) {
    let Some(source) = rfd::FileDialog::new()
        .set_directory(root)
        .add_filter("request", &["json"])
        .pick_file()
    else {
        return;
    };
    let legacy: forge_core::model::RequestDef = match forge_core::store::load_json(&source) {
        Ok(request) => request,
        Err(error) => {
            state.status = Some(StatusMessage::error(error.to_string()));
            return;
        }
    };
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("request");
    let id = stem.strip_suffix(".request").unwrap_or(stem);
    let migrated = match forge_core::reqv1::migrate_request(&legacy, id) {
        Ok(request) => request,
        Err(error) => {
            state.status = Some(StatusMessage::error(error.to_string()));
            return;
        }
    };
    let target = root.join("requests").join(format!("{id}.request.json"));
    if target.exists() {
        state.status = Some(StatusMessage::error(format!(
            "{} already exists",
            target.display()
        )));
        return;
    }
    if let Err(error) = forge_core::store::save_json(&target, &migrated) {
        state.status = Some(StatusMessage::error(error.to_string()));
        return;
    }
    let env = state.active_env.clone();
    match state.dialogs.v1_editor.open_file(target.clone(), env) {
        Ok(()) => {
            state.status = Some(StatusMessage::info(format!(
                "Migrated request to {}",
                target.display()
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error)),
    }
}

fn migrate_legacy_tree(state: &mut AppState, root: &std::path::Path) {
    let Some(source) = rfd::FileDialog::new().pick_folder() else {
        return;
    };
    let destination = root.join("requests");
    let preview = match forge_core::reqv1::migrate_tree(&source, &destination, true) {
        Ok(report) => report,
        Err(error) => {
            state.status = Some(StatusMessage::error(error.to_string()));
            return;
        }
    };
    let ready = preview
        .iter()
        .filter(|item| item.status == forge_core::reqv1::MigrationStatus::Ready)
        .count();
    let blocked = preview.len().saturating_sub(ready);
    let confirmed = rfd::MessageDialog::new()
        .set_title("Migration preview")
        .set_description(format!(
            "{ready} request(s) ready, {blocked} blocked/existing.\nWrite migratable requests now?"
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes;
    if !confirmed {
        state.status = Some(StatusMessage::info(format!(
            "Migration preview: {ready} ready, {blocked} blocked"
        )));
        return;
    }
    match forge_core::reqv1::migrate_tree(&source, &destination, false) {
        Ok(report) => {
            let migrated = report
                .iter()
                .filter(|item| item.status == forge_core::reqv1::MigrationStatus::Migrated)
                .count();
            let blocked = report.len().saturating_sub(migrated);
            state.assets.load(root.to_path_buf());
            state.status = Some(StatusMessage::info(format!(
                "Migrated {migrated} request(s); {blocked} blocked/existing"
            )));
        }
        Err(error) => state.status = Some(StatusMessage::error(error.to_string())),
    }
}

/// Directory of the currently active request tab, if it is a v1 request file
/// — used to make copied relative refs correct for the file being edited.
fn active_request_dir(state: &AppState) -> Option<PathBuf> {
    let idx = state.active_tab?;
    let tab = state.tabs.get(idx)?;
    let path = PathBuf::from(&tab.rel_id);
    // Tabs store a workspace-relative id; join to the workspace root.
    let ws_root = state.workspace.as_ref().map(|w| w.root.clone())?;
    let abs = ws_root.join(path);
    abs.parent().map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_single_safe_path_components() {
        assert!(valid_folder_name("checkout"));
        assert!(valid_folder_name("story 42"));
        assert!(!valid_folder_name(""));
        assert!(!valid_folder_name("../outside"));
        assert!(!valid_folder_name("story/login"));
    }

    #[test]
    fn project_tree_keeps_empty_story_folders() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("requests/checkout")).unwrap();

        let tree = project_tree(root.path()).unwrap();
        let requests = tree
            .iter()
            .find(|node| node.path.ends_with("requests"))
            .unwrap();
        assert!(requests
            .children
            .iter()
            .any(|node| node.path.ends_with("requests/checkout") && node.directory));
    }

    #[test]
    fn project_tree_hides_inherited_property_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let story = root.path().join("requests/checkout");
        std::fs::create_dir_all(&story).unwrap();
        forge_core::reqv1::set_environment(&story, "local").unwrap();
        forge_core::reqv1::set_openapi(&story, "https://example.com/openapi.json").unwrap();

        let tree = project_tree(root.path()).unwrap();
        let requests = tree
            .iter()
            .find(|node| node.path.ends_with("requests"))
            .unwrap();
        let checkout = requests
            .children
            .iter()
            .find(|node| node.path.ends_with("requests/checkout"))
            .unwrap();
        assert!(checkout.children.is_empty());
    }

    #[test]
    fn generated_project_paths_do_not_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sequences");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("sequence.sequence.json"), "{}").unwrap();

        assert_eq!(
            forge_core::reqv1::available_path(&directory, "sequence", ".sequence.json"),
            directory.join("sequence-2.sequence.json")
        );
    }

    #[test]
    fn recursive_beautify_is_all_or_nothing_for_invalid_json() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("story");
        std::fs::create_dir(&child).unwrap();
        let first = root.path().join("a.json");
        let second = child.join("b.json");
        std::fs::write(&first, r#"{"a":1}"#).unwrap();
        std::fs::write(&second, r#"{"b":2}"#).unwrap();

        assert_eq!(beautify_json_tree(root.path()).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "{\n  \"a\": 1\n}\n"
        );

        std::fs::write(&first, r#"{"a":1}"#).unwrap();
        std::fs::write(child.join("invalid.json"), "{").unwrap();
        assert!(beautify_json_tree(root.path()).is_err());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn adding_files_never_overwrites_an_existing_name() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let file = source.path().join("data.json");
        std::fs::write(&file, "new").unwrap();
        std::fs::write(target.path().join("data.json"), "existing").unwrap();

        assert!(copy_files_to_directory(&[file], target.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(target.path().join("data.json")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn scanning_the_fixture_project_populates_the_index() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../forge-core/tests/fixtures/reqv1/project");
        let mut st = AssetsState::default();
        st.load(root);
        assert!(st.is_loaded());
        let index = st.index.as_ref().unwrap();
        assert!(index
            .assets
            .iter()
            .any(|a| a.rel_path == "assets/data/users.json"));
    }
}
