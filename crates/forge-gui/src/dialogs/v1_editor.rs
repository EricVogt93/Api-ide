//! reqv1 request editor: the central editor for authoring a
//! `*.request.json` with *chill* access to the asset store. Store palette on
//! the left (data fixtures, hooks, assertions, extractors, generators,
//! mocks) — click "insert" to add a ready `ref`/`use` to the typed document,
//! so you reference a stored dataset/assertion instead of rewriting it. JSON
//! editor on the right with Validate / Save / Run.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{RichText, TextEdit};
use forge_core::model::Method;
use forge_core::openapi::{ParsedSpec, SpecOperation, SpecResponse};
use forge_core::reqv1::index::AssetEntry;
use forge_core::reqv1::model::{
    BodySpec, BodyType, HeaderSpec, InlineBody, MockDef, PipelineEntry, ValueBinding,
};
use forge_core::reqv1::runner::CatalogPreview;
use forge_core::reqv1::{
    builtin_catalog, find_builtin, AssertionDocument, AssertionEntry, AssetKind, Binding,
    BuiltinDefinition, BuiltinIntent, BuiltinParameter, BuiltinParameterKind, BuiltinTarget,
    HookDocument, ProjectAssetMetadata, ProjectAssetParameter, ProjectAuthConfig, ProjectIndex,
    ResponseView, RunResult, RunStatus,
};

use crate::bridge::{Bridge, Cmd, V1RunItem, V1RunOutput};
use crate::state::AppState;
use crate::theme::icons;
use crate::widgets::code_editor::{
    code_editor_numbered, code_editor_numbered_diagnostic, code_minimap, EditorDiagnostic, Lang,
    CODE_MINIMAP_WIDTH,
};

#[derive(Default)]
pub struct V1EditorState {
    pub open: bool,
    /// File being edited (its parent's project root is derived).
    file: Option<PathBuf>,
    new_file: bool,
    root: Option<PathBuf>,
    text: String,
    validated_text: String,
    validated_document: Option<forge_core::reqv1::RequestDocument>,
    json_diagnostic: Option<EditorDiagnostic>,
    validation_due: Option<Instant>,
    assertions: AssertionDocument,
    hooks: HookDocument,
    project_auth: Option<ProjectAuthConfig>,
    auth_dirty: bool,
    auth_notice: Option<String>,
    auth_setup: AuthSetup,
    auth_request_choice: String,
    auth_draft: AuthDraft,
    dirty: bool,
    auto_save: bool,
    index: Option<ProjectIndex>,
    openapi: Option<ParsedSpec>,
    openapi_source: Option<PathBuf>,
    openapi_error: Option<String>,
    right_panel_open: bool,
    right_tool: RightTool,
    openapi_query: String,
    openapi_filter: OpenApiFilter,
    openapi_operation: Option<String>,
    marked_operations: BTreeSet<String>,
    auto_covered_operations: BTreeSet<String>,
    advisor_config: crate::advisor::AdvisorConfig,
    advisor_question: String,
    advisor_include_response: bool,
    next_advisor_id: u64,
    active_advisor: Option<u64>,
    advisor_answer: Option<String>,
    advisor_error: Option<String>,
    suite_notice: Option<String>,
    suite_error: Option<String>,
    /// JSON tree expansion in the palette (by asset rel_path).
    expanded: HashSet<String>,
    catalog_query: String,
    catalog_intent: Option<String>,
    catalog_view: CatalogView,
    selected_builtin: Option<String>,
    selected_project: Option<String>,
    editing_assertion: Option<usize>,
    editing_hook: Option<usize>,
    scroll_to_catalog_form: bool,
    catalog_inputs: BTreeMap<String, ParameterInput>,
    catalog_error: Option<String>,
    catalog_notice: Option<String>,
    env_name: Option<String>,
    mock: bool,
    allow_project_code: bool,
    /// Vertical splitter: fraction of height given to the request (top).
    split_ratio: f32,
    /// Which results pane is shown in the bottom split.
    result_tab: ResultTab,
    response_raw: bool,
    // Run plumbing.
    next_run_id: u64,
    active_run: Option<u64>,
    in_flight: bool,
    diagnostics: Vec<String>,
    results: Vec<V1RunItem>,
    selected_result: usize,
    last_response: Option<ResponseView>,
    next_preview_id: u64,
    active_preview: Option<u64>,
    preview_in_flight: bool,
    preview: Option<CatalogPreview>,
    preview_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CatalogView {
    #[default]
    Builtins,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RightTool {
    #[default]
    OpenApi,
    ContractTests,
    ApiTests,
    Performance,
    Advisor,
}

impl RightTool {
    fn label(self) -> &'static str {
        match self {
            Self::OpenApi => "OpenAPI",
            Self::ContractTests => "Contract tests",
            Self::ApiTests => "API tests",
            Self::Performance => "Load & performance",
            Self::Advisor => "AI Advisor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OpenApiFilter {
    #[default]
    All,
    Method(Method),
    Headers,
    Query,
    Path,
    Body,
}

impl OpenApiFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All operations",
            Self::Method(method) => method.as_str(),
            Self::Headers => "Has headers",
            Self::Query => "Has query parameters",
            Self::Path => "Has path parameters",
            Self::Body => "Has request body",
        }
    }

    fn matches(self, operation: &SpecOperation) -> bool {
        match self {
            Self::All => true,
            Self::Method(method) => operation.method == method,
            Self::Headers => !operation.header_params.is_empty(),
            Self::Query => !operation.query_params.is_empty(),
            Self::Path => !operation.path_params.is_empty(),
            Self::Body => {
                operation.request_content_type.is_some()
                    || operation.request_schema.is_some()
                    || operation.request_example.is_some()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ParameterSource {
    #[default]
    Literal,
    Binding,
    Environment,
    Runtime,
    Matrix,
    Secret,
}

impl ParameterSource {
    const ALL: [Self; 6] = [
        Self::Literal,
        Self::Binding,
        Self::Environment,
        Self::Runtime,
        Self::Matrix,
        Self::Secret,
    ];
    const TYPED: [Self; 5] = [
        Self::Literal,
        Self::Binding,
        Self::Environment,
        Self::Runtime,
        Self::Matrix,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Literal => "Literal",
            Self::Binding => "Binding",
            Self::Environment => "Environment",
            Self::Runtime => "Runtime",
            Self::Matrix => "Matrix",
            Self::Secret => "Secret",
        }
    }

    fn namespace(self) -> Option<&'static str> {
        match self {
            Self::Literal => None,
            Self::Binding => Some("bindings"),
            Self::Environment => Some("env"),
            Self::Runtime => Some("runtime"),
            Self::Matrix => Some("matrix"),
            Self::Secret => Some("secret"),
        }
    }
}

fn parameter_sources(kind: BuiltinParameterKind) -> &'static [ParameterSource] {
    match kind {
        BuiltinParameterKind::String => &ParameterSource::ALL,
        BuiltinParameterKind::Integer
        | BuiltinParameterKind::Boolean
        | BuiltinParameterKind::Json => &ParameterSource::TYPED,
    }
}

#[derive(Debug, Clone, Default)]
struct ParameterInput {
    source: ParameterSource,
    value: String,
}

#[derive(Debug, Clone, Copy)]
enum InsertTarget {
    Binding,
    Assertion,
    Pipeline,
    Mock,
}

#[derive(Debug, Clone)]
struct PendingInsert {
    target: InsertTarget,
    suggested_name: String,
    snippet: String,
}

#[derive(Debug, Clone)]
struct ParameterDefinition {
    name: String,
    label: String,
    kind: BuiltinParameterKind,
    required: bool,
    default: Option<serde_json::Value>,
    options: Vec<String>,
    example: String,
}

impl ParameterDefinition {
    fn builtin(parameter: &BuiltinParameter) -> Self {
        Self {
            name: parameter.name.to_string(),
            label: parameter.label.to_string(),
            kind: parameter.kind,
            required: parameter.required,
            default: parameter
                .default
                .and_then(|default| serde_json::from_str(default).ok()),
            options: parameter
                .options
                .iter()
                .map(|option| (*option).to_string())
                .collect(),
            example: parameter.example.to_string(),
        }
    }

    fn project(parameter: &ProjectAssetParameter) -> Self {
        Self {
            name: parameter.name.clone(),
            label: parameter.label.clone(),
            kind: parameter.kind,
            required: parameter.required,
            default: parameter.default.clone(),
            options: parameter.options.clone(),
            example: parameter.example.clone(),
        }
    }
}

fn sourced_value(source: ParameterSource, path: &str) -> Option<serde_json::Value> {
    source
        .namespace()
        .filter(|_| !path.trim().is_empty())
        .map(|namespace| serde_json::Value::String(format!("${{{namespace}.{}}}", path.trim())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ResultTab {
    #[default]
    Result,
    Assertions,
    Hooks,
    Auth,
    Runtime,
    Trace,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AuthSetup {
    #[default]
    ExistingRequest,
    Provider,
}

impl AuthSetup {
    fn label(self) -> &'static str {
        match self {
            Self::ExistingRequest => "Existing request",
            Self::Provider => "Provider setup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AuthProvider {
    #[default]
    Generic,
    Keycloak,
    Auth0,
    Entra,
}

impl AuthProvider {
    const ALL: [Self; 4] = [Self::Generic, Self::Keycloak, Self::Auth0, Self::Entra];

    fn label(self) -> &'static str {
        match self {
            Self::Generic => "OAuth 2.0",
            Self::Keycloak => "Keycloak",
            Self::Auth0 => "Auth0",
            Self::Entra => "Microsoft Entra",
        }
    }

    fn endpoint_label(self) -> &'static str {
        match self {
            Self::Generic => "Token URL",
            Self::Keycloak => "Server URL",
            Self::Auth0 => "Domain",
            Self::Entra => "Tenant ID",
        }
    }

    fn scope_label(self) -> &'static str {
        if self == Self::Auth0 {
            "Audience"
        } else {
            "Scope"
        }
    }

    fn secret_name(self) -> &'static str {
        match self {
            Self::Generic => "OAUTH_CLIENT_SECRET",
            Self::Keycloak => "KEYCLOAK_CLIENT_SECRET",
            Self::Auth0 => "AUTH0_CLIENT_SECRET",
            Self::Entra => "ENTRA_CLIENT_SECRET",
        }
    }

    fn file_stem(self) -> &'static str {
        match self {
            Self::Generic => "oauth-token",
            Self::Keycloak => "keycloak-token",
            Self::Auth0 => "auth0-token",
            Self::Entra => "entra-token",
        }
    }
}

#[derive(Debug, Default)]
struct AuthDraft {
    provider: AuthProvider,
    endpoint: String,
    realm: String,
    client_id: String,
    client_secret: String,
    scope: String,
}

impl V1EditorState {
    pub(crate) fn reveal_right_tools(&mut self) {
        self.right_panel_open = true;
        self.right_tool = RightTool::OpenApi;
    }

    pub fn active_file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn last_execution_ms(&self) -> Option<u64> {
        self.last_response.as_ref().map(|response| response.time_ms)
    }

    pub fn has_unsaved_request_under(&self, directory: &Path) -> bool {
        self.dirty
            && self
                .file
                .as_deref()
                .is_some_and(|file| file.starts_with(directory))
    }

    pub fn set_regression(&mut self, file: &Path, enabled: bool) -> Result<bool, String> {
        if self.file.as_deref() != Some(file) {
            return Ok(false);
        }
        let mut document = forge_core::reqv1::RequestDocument::parse(&self.text)
            .map_err(|error| format!("invalid request JSON: {error}"))?;
        document.meta.set_regression(enabled);
        self.text = serialize_request(&document)?;
        self.dirty = true;
        if save_now(self) {
            Ok(true)
        } else {
            Err(self
                .diagnostics
                .first()
                .cloned()
                .unwrap_or_else(|| "failed to save regression property".to_string()))
        }
    }

    pub fn reload_clean_request_under(&mut self, directory: &Path) -> Result<(), String> {
        let Some(file) = self
            .file
            .as_deref()
            .filter(|file| file.starts_with(directory) && file.is_file())
        else {
            return Ok(());
        };
        self.text = std::fs::read_to_string(file)
            .map_err(|error| format!("failed to reload {}: {error}", file.display()))?;
        validate_editor_json(self);
        self.clear_preview();
        Ok(())
    }

    /// Open the editor on `file` (an existing document). Rescans its project.
    pub fn open_file(&mut self, file: PathBuf, active_env: Option<String>) -> Result<(), String> {
        if self.auto_save && self.dirty && self.file.as_ref() != Some(&file) && !save_now(self) {
            return Err("auto-save failed; the current request remains open".to_string());
        }
        let mut text = std::fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let mut assertions = AssertionDocument::load_for_request(&file)?;
        let mut hooks = HookDocument::load_for_request(&file)?;
        let mut migrated = false;
        if let Ok(mut request) = forge_core::reqv1::RequestDocument::parse(&text) {
            let inline = AssertionDocument::take_from_request(&mut request);
            let inline_hooks = HookDocument::take_from_request(&mut request);
            if !inline.assertions.is_empty() {
                assertions.extend(inline);
                migrated = true;
            }
            if !inline_hooks.hooks.is_empty() {
                hooks.extend(inline_hooks);
                migrated = true;
            }
            if migrated {
                text = serialize_request(&request)?;
            }
        }
        self.text = text;
        validate_editor_json(self);
        self.assertions = assertions;
        self.hooks = hooks;
        self.root = Some(project_root_of(&file));
        self.prepare_right_tools();
        self.env_name = active_env;
        self.file = Some(file);
        self.new_file = false;
        self.dirty = migrated;
        self.active_run = None;
        self.in_flight = false;
        self.results.clear();
        self.selected_result = 0;
        self.last_response = None;
        self.result_tab = ResultTab::Result;
        self.editing_assertion = None;
        self.editing_hook = None;
        self.auth_dirty = false;
        self.auth_notice = None;
        self.allow_project_code = false;
        self.clear_preview();
        self.diagnostics = self.load_index().err().into_iter().collect();
        self.right_tool = if self.openapi.is_some() {
            RightTool::OpenApi
        } else {
            RightTool::Advisor
        };
        if self.split_ratio <= 0.0 {
            self.split_ratio = 0.6;
        }
        self.open = true;
        Ok(())
    }

    /// Open a new skeleton request at the next free conventional project path.
    pub fn open_new(&mut self, root: PathBuf, active_env: Option<String>) {
        let directory = root.join("requests");
        self.open_new_in(root, directory, active_env);
    }

    /// Open a new request inside a selected story folder under `requests/`.
    pub fn open_new_in(&mut self, root: PathBuf, directory: PathBuf, active_env: Option<String>) {
        if self.auto_save && self.dirty && !save_now(self) {
            return;
        }
        let requests = root.join("requests");
        let directory = if directory.starts_with(&requests) {
            directory
        } else {
            requests
        };
        self.text = SKELETON.to_string();
        validate_editor_json(self);
        self.assertions = AssertionDocument::default();
        self.hooks = HookDocument::default();
        self.file = Some(forge_core::reqv1::available_path(
            &directory,
            "new",
            ".request.json",
        ));
        self.new_file = true;
        self.root = Some(root);
        self.prepare_right_tools();
        self.env_name = active_env;
        self.dirty = true;
        self.active_run = None;
        self.in_flight = false;
        self.results.clear();
        self.selected_result = 0;
        self.last_response = None;
        self.result_tab = ResultTab::Result;
        self.editing_assertion = None;
        self.editing_hook = None;
        self.auth_dirty = false;
        self.auth_notice = None;
        self.allow_project_code = false;
        self.clear_preview();
        self.diagnostics = self.load_index().err().into_iter().collect();
        self.right_tool = if self.openapi.is_some() {
            RightTool::OpenApi
        } else {
            RightTool::Advisor
        };
        if self.split_ratio <= 0.0 {
            self.split_ratio = 0.6;
        }
        self.open = true;
    }

    fn load_index(&mut self) -> Result<(), String> {
        let root = self.root.clone();
        if !self.auth_dirty {
            self.project_auth = match root.as_deref() {
                Some(root) => {
                    forge_core::reqv1::load_project(root)
                        .map_err(|diagnostic| diagnostic.message)?
                        .auth
                }
                None => None,
            };
        }
        self.index = match root.as_ref() {
            Some(root) => Some(
                ProjectIndex::scan(root)
                    .map_err(|diagnostic| format!("asset index: {}", diagnostic.message))?,
            ),
            None => None,
        };
        let (openapi_source, openapi, openapi_error) =
            root.as_deref().map(discover_openapi).unwrap_or_default();
        self.openapi_source = openapi_source;
        self.openapi = openapi;
        self.openapi_error = openapi_error;
        self.auto_covered_operations =
            scan_covered_operations(root.as_deref(), self.index.as_ref(), self.openapi.as_ref());
        self.marked_operations = root
            .as_deref()
            .and_then(|root| load_marked_operations(root).ok())
            .unwrap_or_default();
        Ok(())
    }

    /// Route a bridge `Evt::V1Run` outcome.
    pub fn handle_result(&mut self, run_id: u64, result: Result<V1RunOutput, String>) {
        if self.active_run != Some(run_id) {
            return;
        }
        self.in_flight = false;
        self.active_run = None;
        self.clear_preview();
        match result {
            Ok(output) => {
                self.results = output.items;
                self.selected_result = 0;
                self.last_response = self.results.first().and_then(|item| item.response.clone());
                self.result_tab = ResultTab::Result;
            }
            Err(e) => {
                self.diagnostics = vec![e];
                self.result_tab = ResultTab::Diagnostics;
            }
        }
    }

    /// Route a bridge `Evt::V1Preview` outcome and ignore stale replies.
    pub fn handle_preview(&mut self, preview_id: u64, result: Result<CatalogPreview, String>) {
        if self.active_preview != Some(preview_id) {
            return;
        }
        self.preview_in_flight = false;
        self.active_preview = None;
        match result {
            Ok(preview) => {
                self.preview = Some(preview);
                self.preview_error = None;
            }
            Err(error) => {
                self.preview = None;
                self.preview_error = Some(error);
            }
        }
    }

    pub fn handle_advisor(&mut self, advisor_id: u64, result: Result<String, String>) {
        if self.active_advisor != Some(advisor_id) {
            return;
        }
        self.active_advisor = None;
        match result {
            Ok(answer) => {
                self.advisor_answer = Some(answer);
                self.advisor_error = None;
            }
            Err(error) => {
                self.advisor_answer = None;
                self.advisor_error = Some(error);
            }
        }
    }

    fn prepare_right_tools(&mut self) {
        self.right_panel_open = true;
        self.openapi_query.clear();
        self.openapi_filter = OpenApiFilter::All;
        self.openapi_operation = None;
        self.advisor_include_response = false;
        self.active_advisor = None;
        self.advisor_answer = None;
        self.advisor_error = None;
        self.suite_notice = None;
        self.suite_error = None;
        if self.advisor_question.is_empty() {
            self.advisor_question =
                "Review this request against its OpenAPI contract and suggest concrete fixes."
                    .to_string();
        }
        if let Some(root) = &self.root {
            match crate::advisor::load(root) {
                Ok(config) => self.advisor_config = config,
                Err(error) => self.advisor_error = Some(error),
            }
        }
    }

    fn clear_preview(&mut self) {
        self.active_preview = None;
        self.preview_in_flight = false;
        self.preview = None;
        self.preview_error = None;
    }

    /// Run every request that uses a selected asset as independent matrix
    /// runs and show the combined result in this editor.
    pub fn run_affected(
        &mut self,
        root: PathBuf,
        files: Vec<PathBuf>,
        active_env: Option<String>,
        bridge: &Bridge,
    ) {
        if self.root.as_ref() != Some(&root) {
            self.open_new(root.clone(), active_env.clone());
        } else {
            self.open = true;
        }
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        self.active_run = Some(run_id);
        self.in_flight = true;
        self.results.clear();
        self.selected_result = 0;
        self.last_response = None;
        self.diagnostics.clear();
        self.result_tab = ResultTab::Result;
        if let Err(error) = bridge.send(Cmd::RunV1Batch {
            run_id,
            root,
            files,
            env_name: active_env,
            mock: self.mock,
            allow_project_code: self.allow_project_code,
        }) {
            self.active_run = None;
            self.in_flight = false;
            self.diagnostics = vec![error];
            self.result_tab = ResultTab::Diagnostics;
        }
    }

    /// Run a persisted sequence after its document has resolved the ordered
    /// request paths.
    pub fn run_sequence(
        &mut self,
        root: PathBuf,
        files: Vec<PathBuf>,
        active_env: Option<String>,
        bridge: &Bridge,
    ) {
        if self.root.as_ref() != Some(&root) {
            self.open_new(root.clone(), active_env.clone());
        } else {
            self.open = true;
        }
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        self.active_run = Some(run_id);
        self.in_flight = true;
        self.results.clear();
        self.selected_result = 0;
        self.last_response = None;
        self.diagnostics.clear();
        self.result_tab = ResultTab::Result;
        if let Err(error) = bridge.send(Cmd::RunV1Sequence {
            run_id,
            root,
            files,
            env_name: active_env,
            mock: self.mock,
            allow_project_code: self.allow_project_code,
        }) {
            self.active_run = None;
            self.in_flight = false;
            self.diagnostics = vec![error];
            self.result_tab = ResultTab::Diagnostics;
        }
    }
}

const SKELETON: &str = r#"{
  "formatVersion": 1,
  "kind": "request",
  "meta": { "id": "new.request", "name": "New request" },
  "bindings": {
  },
  "request": {
    "method": "GET",
    "url": "https://example.com",
    "headers": []
  }
}
"#;

const EDITOR_COLUMN_GAP: f32 = 22.0;
const TOOLBAR_MENU_CELL_WIDTH: f32 = 40.0;
const TOOLBAR_TRAILING_GUTTER: f32 = 8.0;

fn editor_column_widths(available_width: f32) -> (f32, f32) {
    let editor_area_width = available_width.max(0.0);
    let usable_width = (editor_area_width - EDITOR_COLUMN_GAP).max(0.0);
    let catalog_width = (editor_area_width * 0.34)
        .clamp(250.0, 330.0)
        .min((usable_width - 280.0).max(0.0))
        .min(usable_width);
    let request_width = usable_width - catalog_width;
    (catalog_width, request_width)
}

fn zoom_editor_font_size(current: f32, zoom_delta: f32) -> f32 {
    (current * zoom_delta).clamp(9.0, 24.0)
}

/// Render the central v1 editor if open.
pub fn show(ui: &mut egui::Ui, state: &mut AppState, bridge: &Bridge) {
    if !state.dialogs.v1_editor.open {
        return;
    }
    let mut pending_insert: Option<PendingInsert> = None;
    let mut refresh_project = false;
    let mut zoom_target_hovered = false;
    let accent = state.theme.accent_color();
    let mut editor_font_size = state.editor_font_size;
    let auto_save = state.auto_save;
    let show_right_tools = !state.zen_mode || state.zen_right_revealed;
    let show_catalog = !state.zen_mode || state.zen_left_revealed;
    let configured_openapi = state.openapi.clone();
    let configured_openapi_source = state.openapi_source.clone();
    let configured_openapi_error = state.openapi_error.clone();
    let selected_project_dir = state.assets.selected_directory();
    let d = &mut state.dialogs.v1_editor;
    d.auto_save = auto_save;
    if let Some(source) = configured_openapi_source {
        let source = PathBuf::from(source);
        if d.openapi_source.as_ref() != Some(&source)
            || (d.openapi.is_none() && configured_openapi.is_some())
            || d.openapi_error != configured_openapi_error
        {
            d.openapi_source = Some(source);
            d.openapi = configured_openapi;
            d.openapi_error = configured_openapi_error;
            d.auto_covered_operations =
                scan_covered_operations(d.root.as_deref(), d.index.as_ref(), d.openapi.as_ref());
        }
    }
    refresh_editor_validation(d, ui.ctx());
    let surface_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    egui::Frame::NONE
        .fill(ui.visuals().panel_fill)
        .stroke(surface_stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("REQUEST").small().strong().color(accent));
                ui.label(
                    RichText::new(
                        d.root
                            .as_deref()
                            .zip(d.file.as_deref())
                            .and_then(|(root, file)| file.strip_prefix(root).ok())
                            .unwrap_or_else(|| d.file.as_deref().unwrap_or_else(|| Path::new("")))
                            .display()
                            .to_string(),
                    )
                    .strong(),
                );
                if d.dirty {
                    ui.label(RichText::new(icons::DIRTY).color(accent));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(icons::CLOSE)
                        .on_hover_text("Close request")
                        .clicked()
                        && (!d.auto_save || !d.dirty || save_now(d))
                    {
                        d.open = false;
                    }
                });
            });
        });
    ui.add_space(8.0);

    if show_right_tools {
        let mut right_panel_open = d.right_panel_open;
        let collapsed_panel = egui::Panel::right("v1-right-tools-collapsed")
            .exact_size(38.0)
            .resizable(true);
        let expanded_panel = egui::Panel::right("v1-right-tools-expanded")
            .default_size(300.0)
            .resizable(true)
            .size_range(240.0..=460.0);
        let (requested_open, generated_suite) = egui::Panel::show_switched(
            ui,
            &mut right_panel_open,
            collapsed_panel,
            expanded_panel,
            |ui, expanded| right_sidebar(ui, d, bridge, selected_project_dir.as_deref(), expanded),
        )
        .inner;
        if let Some(open) = requested_open {
            right_panel_open = open;
        }
        d.right_panel_open = right_panel_open;
        refresh_project |= generated_suite;
    }

    egui::CentralPanel::no_frame().show(ui, |ui| {
        let body_size = ui.available_size();
        let (catalog_width, request_width) = if show_catalog {
            editor_column_widths(body_size.x)
        } else {
            (0.0, body_size.x)
        };
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
            if show_catalog {
                ui.allocate_ui_with_layout(
                    egui::vec2(catalog_width, body_size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::Frame::NONE
                            .fill(ui.visuals().panel_fill)
                            .stroke(surface_stroke)
                            .corner_radius(8)
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_min_height((body_size.y - 24.0).max(0.0));
                                ui.label(RichText::new("Catalog").size(18.0).strong())
                                    .on_hover_text(
                                    "Configure reusable behavior once and insert typed references.",
                                );
                                ui.add_space(8.0);
                                palette(ui, d, bridge, &mut pending_insert);
                            });
                    },
                );
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(request_width, body_size.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 48.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::NONE
                                .fill(ui.visuals().panel_fill)
                                .stroke(surface_stroke)
                                .corner_radius(8)
                                .inner_margin(egui::Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    let compact_toolbar = ui.available_width() < 620.0;
                                    let can_run = !d.in_flight && d.root.is_some();
                                    egui_extras::StripBuilder::new(ui)
                                        .size(egui_extras::Size::remainder())
                                        .size(egui_extras::Size::exact(TOOLBAR_MENU_CELL_WIDTH))
                                        .size(egui_extras::Size::exact(TOOLBAR_TRAILING_GUTTER))
                                        .clip(true)
                                        .horizontal(|mut strip| {
                                            strip.cell(|ui| {
                                                ui.horizontal(|ui| {
                                                    let run_label = if document_has_matrix(&d.text)
                                                    {
                                                        format!("{}  Run matrix", icons::PLAY)
                                                    } else {
                                                        format!("{}  Run", icons::PLAY)
                                                    };
                                                    if ui
                                                        .add_enabled(
                                                            can_run,
                                                            crate::theme::primary_button(
                                                                run_label, accent,
                                                            ),
                                                        )
                                                        .on_hover_text(if document_has_matrix(&d.text) {
                                                            "Execute every case in the request matrix"
                                                        } else {
                                                            "Execute the active request"
                                                        })
                                                        .clicked()
                                                    {
                                                        run_now(d, bridge);
                                                    }
                                                    if ui
                                                        .button(format!("{}  Save", icons::SAVE))
                                                        .on_hover_text("Save the request and its assertion and hook sidecars")
                                                        .clicked()
                                                    {
                                                        refresh_project = save_now(d);
                                                    }
                                                    if !compact_toolbar {
                                                        if ui.button("Format")
                                                            .on_hover_text("Beautify the request JSON")
                                                            .clicked() {
                                                            format_request(d);
                                                        }
                                                        if ui.button("Validate")
                                                            .on_hover_text("Validate JSON, references and OpenAPI compatibility")
                                                            .clicked() {
                                                            validate_now(d);
                                                        }
                                                    }

                                                    let envs: Vec<String> = d
                                                        .index
                                                        .as_ref()
                                                        .map(|index| index.environments.clone())
                                                        .unwrap_or_default();
                                                    let inherited =
                                                d.root.as_deref().zip(d.file.as_deref()).and_then(
                                                    |(root, file)| {
                                                        forge_core::reqv1::effective_environment(
                                                            root, file,
                                                        )
                                                        .ok()
                                                        .flatten()
                                                    },
                                                );
                                                    let selected =
                                                        d.env_name.clone().unwrap_or_else(|| {
                                                            inherited
                                                                .as_ref()
                                                                .map(|selection| {
                                                                    format!(
                                                                        "{} · inherited",
                                                                        selection.value
                                                                    )
                                                                })
                                                                .unwrap_or_else(|| {
                                                                    "Automatic · none".to_string()
                                                                })
                                                        });
                                                    let previous_env = d.env_name.clone();
                                                    egui::ComboBox::from_id_salt("v1-env")
                                                        .selected_text(selected)
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(
                                                                &mut d.env_name,
                                                                None,
                                                                "Automatic (properties)",
                                                            );
                                                            for env in &envs {
                                                                ui.selectable_value(
                                                                    &mut d.env_name,
                                                                    Some(env.clone()),
                                                                    env,
                                                                );
                                                            }
                                                        });
                                                    ui.response().on_hover_text("Override the environment inherited from project properties");
                                                    if d.env_name != previous_env {
                                                        d.clear_preview();
                                                    }
                                                    if d.in_flight {
                                                        ui.spinner();
                                                    }
                                                });
                                            });
                                            strip.cell(|ui| {
                                                ui.centered_and_justified(|ui| {
                                                    ui.menu_button(icons::ELLIPSIS, |ui| {
                                                        if compact_toolbar {
                                                            if ui.button("Format")
                                                                .on_hover_text("Beautify the request JSON")
                                                                .clicked() {
                                                                format_request(d);
                                                                ui.close();
                                                            }
                                                            if ui.button("Validate")
                                                                .on_hover_text("Validate JSON, references and OpenAPI compatibility")
                                                                .clicked() {
                                                                validate_now(d);
                                                                ui.close();
                                                            }
                                                            ui.separator();
                                                        }
                                                        ui.checkbox(
                                                            &mut d.mock,
                                                            "Use mock response",
                                                        )
                                                        .on_hover_text("Run the request against its deterministic mock instead of the network");
                                                        ui.checkbox(
                                                    &mut d.allow_project_code,
                                                    "Allow project code",
                                                )
                                                .on_hover_text(
                                                    "Executes reviewed project-owned JavaScript.",
                                                );
                                                        ui.separator();
                                                        if ui
                                                            .add_enabled(
                                                                can_run,
                                                                egui::Button::new("Run sequence…"),
                                                            )
                                                            .on_hover_text("Choose and execute an ordered request sequence")
                                                            .clicked()
                                                        {
                                                            run_sequence_now(d, bridge);
                                                            ui.close();
                                                        }
                                                    })
                                                    .response
                                                    .on_hover_text("Run mode and additional editor actions");
                                                });
                                            });
                                            strip.empty();
                                        });
                                });
                        },
                    );
                    ui.add_space(8.0);

                    let total_h = ui.available_height();
                    let top_h =
                        (total_h * d.split_ratio).clamp(180.0, (total_h - 120.0).max(180.0));
                    ui.allocate_ui(egui::vec2(ui.available_width(), top_h), |ui| {
                        ui.label(RichText::new("REQUEST").small().strong().weak());
                        let assist_height = request_editor_footer_height(d);
                        let editor_height = (ui.available_height() - assist_height).max(120.0);
                        let editor_content_height = (editor_height - 16.0).max(104.0);
                        let editor_frame = egui::Frame::NONE
                            .fill(ui.visuals().extreme_bg_color)
                            .stroke(surface_stroke)
                            .corner_radius(6)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_min_height(editor_content_height);
                                let diagnostic = d.json_diagnostic.clone();
                                let mut editor_response = None;
                                egui_extras::StripBuilder::new(ui)
                                    .size(egui_extras::Size::remainder())
                                    .size(egui_extras::Size::exact(CODE_MINIMAP_WIDTH))
                                    .horizontal(|mut strip| {
                                        strip.cell(|ui| {
                                            editor_response = Some(
                                                egui::ScrollArea::both()
                                                    .id_salt("v1-json")
                                                    .max_height(editor_content_height)
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        code_editor_numbered_diagnostic(
                                                            ui,
                                                            "v1-request-json",
                                                            &mut d.text,
                                                            Lang::Json,
                                                            None,
                                                            false,
                                                            18,
                                                            false,
                                                            diagnostic.as_ref(),
                                                        )
                                                    })
                                                    .inner,
                                            );
                                        });
                                        strip.cell(|ui| {
                                            code_minimap(
                                                ui,
                                                &d.text,
                                                diagnostic.as_ref(),
                                                ui.available_height(),
                                            );
                                        });
                                    });
                                editor_response.unwrap_or_else(|| {
                                    ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
                                })
                            });
                        zoom_target_hovered |= ui.rect_contains_pointer(editor_frame.response.rect);
                        let response = editor_frame.inner;
                        if response.changed() {
                            d.dirty = true;
                            d.clear_preview();
                            schedule_editor_validation(d, ui.ctx());
                        }
                        if let Some(diagnostic) = &d.json_diagnostic {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                format!(
                                    "Line {}, column {}: {}",
                                    diagnostic.line, diagnostic.column, diagnostic.message
                                ),
                            );
                        }
                        openapi_assist(ui, d, &response);
                    });

                    let splitter = ui.allocate_response(
                        egui::vec2(ui.available_width(), 8.0),
                        egui::Sense::drag(),
                    );
                    ui.painter().hline(
                        splitter.rect.x_range(),
                        splitter.rect.center().y,
                        ui.visuals().widgets.noninteractive.bg_stroke,
                    );
                    if splitter.dragged() && total_h > 1.0 {
                        d.split_ratio =
                            ((top_h + splitter.drag_delta().y) / total_h).clamp(0.2, 0.85);
                    }
                    if splitter.hovered() || splitter.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    }

                    let results = ui.allocate_ui(
                        egui::vec2(ui.available_width(), ui.available_height()),
                        |ui| results_pane(ui, d, editor_font_size),
                    );
                    zoom_target_hovered |= ui.rect_contains_pointer(results.response.rect);
                },
            );
        });
    });

    if zoom_target_hovered {
        let zoom_delta = ui.input(|input| {
            if input.modifiers.ctrl {
                input.zoom_delta()
            } else {
                1.0
            }
        });
        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            editor_font_size = zoom_editor_font_size(editor_font_size, zoom_delta);
            ui.ctx().request_repaint();
        }
    }

    if let Some(insert) = pending_insert {
        match apply_insert(d, insert) {
            Ok((text, notice)) => {
                if let Some(text) = text {
                    d.text = text;
                }
                d.dirty = true;
                d.catalog_error = None;
                d.catalog_notice = Some(notice);
                d.clear_preview();
            }
            Err(error) => d.catalog_error = Some(error),
        }
    }
    if refresh_project {
        if let Some(root) = state.assets.project_root() {
            state.assets.load(root);
        }
    }
    if (editor_font_size - state.editor_font_size).abs() > f32::EPSILON {
        state.editor_font_size = editor_font_size;
        crate::dialogs::settings::apply_typography(ui.ctx(), state);
    }
}

fn right_sidebar(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    bridge: &Bridge,
    selected_directory: Option<&Path>,
    expanded: bool,
) -> (Option<bool>, bool) {
    if !expanded {
        let mut requested_open = None;
        ui.vertical_centered(|ui| {
            if ui
                .small_button(icons::CODE)
                .on_hover_text("OpenAPI")
                .clicked()
            {
                d.right_tool = RightTool::OpenApi;
                requested_open = Some(true);
            }
            if ui
                .small_button(icons::CHECK)
                .on_hover_text("Contract tests")
                .clicked()
            {
                d.right_tool = RightTool::ContractTests;
                requested_open = Some(true);
            }
            if ui
                .small_button(icons::RUN)
                .on_hover_text("API tests")
                .clicked()
            {
                d.right_tool = RightTool::ApiTests;
                requested_open = Some(true);
            }
            if ui
                .small_button(icons::PULSE)
                .on_hover_text("Load & performance")
                .clicked()
            {
                d.right_tool = RightTool::Performance;
                requested_open = Some(true);
            }
            if ui
                .small_button(icons::CONSOLE)
                .on_hover_text("AI Advisor")
                .clicked()
            {
                d.right_tool = RightTool::Advisor;
                requested_open = Some(true);
            }
        });
        return (requested_open, false);
    }

    let mut requested_open = None;
    let mut generated_suite = false;
    egui::Frame::NONE
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_height((ui.available_height() - 24.0).max(0.0));
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                for (tool, icon) in [
                    (RightTool::OpenApi, icons::CODE),
                    (RightTool::ContractTests, icons::CHECK),
                    (RightTool::ApiTests, icons::RUN),
                    (RightTool::Performance, icons::PULSE),
                    (RightTool::Advisor, icons::CONSOLE),
                ] {
                    let active = d.right_tool == tool;
                    if ui
                        .selectable_label(active, icon)
                        .on_hover_text(tool.label())
                        .clicked()
                    {
                        d.right_tool = tool;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(icons::TRIANGLE_RIGHT)
                        .on_hover_text("Collapse tool window")
                        .clicked()
                    {
                        requested_open = Some(false);
                    }
                });
            });
            ui.separator();
            ui.add_space(8.0);
            match d.right_tool {
                RightTool::OpenApi => openapi_sidebar(ui, d),
                RightTool::ContractTests => {
                    generated_suite |= generated_suite_sidebar(
                        ui,
                        d,
                        selected_directory,
                        forge_core::reqv1::OpenApiSuiteKind::Contract,
                    )
                }
                RightTool::ApiTests => {
                    generated_suite |= generated_suite_sidebar(
                        ui,
                        d,
                        selected_directory,
                        forge_core::reqv1::OpenApiSuiteKind::Api,
                    )
                }
                RightTool::Performance => {
                    generated_suite |= generated_suite_sidebar(
                        ui,
                        d,
                        selected_directory,
                        forge_core::reqv1::OpenApiSuiteKind::K6,
                    )
                }
                RightTool::Advisor => advisor_sidebar(ui, d, bridge),
            }
        });
    (requested_open, generated_suite)
}

fn generated_suite_sidebar(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    selected_directory: Option<&Path>,
    kind: forge_core::reqv1::OpenApiSuiteKind,
) -> bool {
    let (title, action, tooltip) = match kind {
        forge_core::reqv1::OpenApiSuiteKind::Contract => (
            "Contract tests",
            "Generate contract tests",
            "Creates runnable requests with status, content-type and response-schema assertions.",
        ),
        forge_core::reqv1::OpenApiSuiteKind::Api => (
            "API tests",
            "Generate API tests",
            "Creates one complete request per operation, assertion sidecars and an ordered sequence.",
        ),
        forge_core::reqv1::OpenApiSuiteKind::K6 => (
            "k6 performance",
            "Generate k6 suite",
            "Creates smoke, load, stress, spike and soak profiles. Mutating methods are disabled by default.",
        ),
    };
    ui.strong(title);
    let target = selected_directory
        .map(Path::to_path_buf)
        .or_else(|| {
            d.file
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .or_else(|| d.root.as_ref().map(|root| root.join("requests")));
    if let Some(target) = &target {
        let shown = d
            .root
            .as_deref()
            .and_then(|root| target.strip_prefix(root).ok())
            .unwrap_or(target)
            .join(kind.folder());
        ui.monospace(shown.display().to_string());
    }
    ui.add_space(10.0);

    let ready = d.openapi.is_some() && d.root.is_some() && target.is_some();
    let clicked = ui
        .add_enabled(ready, egui::Button::new(action))
        .on_hover_text(tooltip)
        .clicked();
    if !ready {
        ui.weak("OpenAPI and a project folder are required.");
    }
    if let Some(error) = &d.suite_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(notice) = &d.suite_notice {
        ui.colored_label(ui.visuals().hyperlink_color, notice);
    }
    if !clicked {
        return false;
    }

    let result = forge_core::reqv1::generate_openapi_suite(
        d.root.as_deref().expect("ready checked"),
        target.as_deref().expect("ready checked"),
        d.openapi.as_ref().expect("ready checked"),
        kind,
    );
    match result {
        Ok(generated) => {
            let warning = if generated.warnings.is_empty() {
                String::new()
            } else {
                format!(" · {} warning(s)", generated.warnings.len())
            };
            d.suite_notice = Some(format!(
                "Generated {} request(s), {} file(s){warning}",
                generated.requests, generated.files
            ));
            d.suite_error = None;
            true
        }
        Err(error) => {
            d.suite_notice = None;
            d.suite_error = Some(error);
            false
        }
    }
}

fn openapi_sidebar(ui: &mut egui::Ui, d: &mut V1EditorState) {
    if let Some(error) = &d.openapi_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
        return;
    }
    let Some(spec) = &d.openapi else {
        ui.strong("No OpenAPI spec found").on_hover_text(
            "Set a source in folder properties or add openapi.json/yaml below the project.",
        );
        return;
    };

    let title = spec.title.clone();
    let version = spec.version.clone();
    let servers = spec.servers.clone();
    let operations = spec.operations.clone();
    let docs_url = openapi_docs_url(spec).map(str::to_string);
    ui.strong(title);
    ui.label(RichText::new(format!("v{version} · {} operations", operations.len())).weak());
    if let Some(server) = servers.first() {
        ui.monospace(server);
    }
    ui.horizontal(|ui| {
        if let Some(source) = &d.openapi_source {
            if ui.small_button("Open spec").clicked() {
                let _ = open::that(source);
            }
        }
        if let Some(url) = docs_url {
            if ui.small_button("Open Swagger UI").clicked() {
                let _ = open::that(url);
            }
        }
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(icons::SEARCH);
        ui.add(
            TextEdit::singleline(&mut d.openapi_query)
                .hint_text("Filter operations")
                .desired_width(ui.available_width()),
        );
    });
    egui::ComboBox::from_id_salt("openapi-operation-filter")
        .selected_text(d.openapi_filter.label())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut d.openapi_filter, OpenApiFilter::All, "All operations");
            ui.separator();
            for method in Method::ALL {
                ui.selectable_value(
                    &mut d.openapi_filter,
                    OpenApiFilter::Method(method),
                    method.as_str(),
                );
            }
            ui.separator();
            for filter in [
                OpenApiFilter::Headers,
                OpenApiFilter::Query,
                OpenApiFilter::Path,
                OpenApiFilter::Body,
            ] {
                ui.selectable_value(&mut d.openapi_filter, filter, filter.label());
            }
        });

    let query = d.openapi_query.trim().to_ascii_lowercase();
    let mut filtered = operations
        .iter()
        .filter(|operation| {
            d.openapi_filter.matches(operation) && operation_matches_query(operation, &query)
        })
        .cloned()
        .collect::<Vec<_>>();
    filtered.sort_by(|left, right| {
        method_rank(left.method)
            .cmp(&method_rank(right.method))
            .then_with(|| left.path.cmp(&right.path))
    });

    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .id_salt("openapi-operations")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut last_method = None;
            for operation in &filtered {
                if last_method != Some(operation.method) {
                    if last_method.is_some() {
                        ui.add_space(6.0);
                    }
                    ui.label(RichText::new(operation.method.as_str()).small().strong());
                    ui.separator();
                    last_method = Some(operation.method);
                }
                let auto_covered = d.auto_covered_operations.contains(&operation.id)
                    || request_covers_operation(&d.text, operation);
                let covered = auto_covered || d.marked_operations.contains(&operation.id);
                egui::Frame::NONE
                    .fill(ui.visuals().faint_bg_color)
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(9))
                    .show(ui, |ui| {
                        let mark_response = ui
                            .horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{}  {}",
                                        operation.method.as_str(),
                                        operation.path
                                    ))
                                    .monospace()
                                    .strong()
                                    .color(method_color(operation.method)),
                                );
                                ui.label(
                                    RichText::new(format!("[{}]", operation_rule(operation)))
                                        .small()
                                        .monospace()
                                        .weak(),
                                )
                                .on_hover_text(operation_rule_help(operation));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let color = if covered {
                                            if ui.visuals().dark_mode {
                                                crate::theme::darcula::OK
                                            } else {
                                                crate::theme::light::OK
                                            }
                                        } else {
                                            ui.visuals().weak_text_color()
                                        };
                                        ui.add_sized(
                                            [24.0, 24.0],
                                            egui::Button::new(
                                                RichText::new(icons::CHECK).strong().color(color),
                                            )
                                            .frame(false),
                                        )
                                    },
                                )
                                .inner
                            })
                            .inner
                            .on_hover_text(if auto_covered {
                                "Covered by the current request"
                            } else if covered {
                                "Marked as covered · click to clear"
                            } else {
                                "Mark as covered"
                            });
                        if mark_response.clicked() && !auto_covered {
                            if !d.marked_operations.remove(&operation.id) {
                                d.marked_operations.insert(operation.id.clone());
                            }
                            if let Some(root) = &d.root {
                                if let Err(error) =
                                    save_marked_operations(root, &d.marked_operations)
                                {
                                    d.diagnostics = vec![error];
                                }
                            }
                        }
                        if !operation.summary.is_empty() {
                            ui.label(&operation.summary);
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("Add to request").clicked() {
                                apply_openapi_to_editor(d, operation, false);
                            }
                            if ui.small_button("Generate custom value").clicked() {
                                apply_openapi_to_editor(d, operation, true);
                            }
                        });
                    });
                ui.add_space(6.0);
            }
            if filtered.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No matching operations");
                });
            }
        });
}

fn method_rank(method: Method) -> usize {
    Method::ALL
        .iter()
        .position(|candidate| *candidate == method)
        .unwrap_or(usize::MAX)
}

fn operation_rule(operation: &SpecOperation) -> &'static str {
    if operation.request_schema.is_some() {
        "sch"
    } else if !operation.path_params.is_empty()
        || operation.query_params.iter().any(|(_, required)| *required)
        || operation
            .header_params
            .iter()
            .any(|(_, required)| *required)
    {
        "req"
    } else {
        "opt"
    }
}

fn operation_rule_help(operation: &SpecOperation) -> &'static str {
    match operation_rule(operation) {
        "sch" => "Request body is constrained by a schema",
        "req" => "Operation has required parameters",
        _ => "Operation has no required input",
    }
}

fn request_covers_operation(text: &str, operation: &SpecOperation) -> bool {
    forge_core::reqv1::RequestDocument::parse(text)
        .ok()
        .is_some_and(|document| {
            document.request.method == operation.method
                && forge_core::openapi::path_matches_template(
                    &operation.path,
                    &forge_core::openapi::url_to_path(&document.request.url),
                )
        })
}

fn scan_covered_operations(
    root: Option<&Path>,
    index: Option<&ProjectIndex>,
    spec: Option<&ParsedSpec>,
) -> BTreeSet<String> {
    let (Some(root), Some(index), Some(spec)) = (root, index, spec) else {
        return BTreeSet::new();
    };
    index
        .requests
        .iter()
        .filter_map(|request| std::fs::read_to_string(root.join(&request.rel_path)).ok())
        .filter_map(|text| forge_core::reqv1::RequestDocument::parse(&text).ok())
        .filter_map(|document| {
            spec.find_operation(document.request.method, &document.request.url)
                .map(|operation| operation.id.clone())
        })
        .collect()
}

fn apply_openapi_to_editor(d: &mut V1EditorState, operation: &SpecOperation, generated: bool) {
    let result = apply_openapi_operation(&d.text, operation).and_then(|text| {
        if generated {
            apply_generated_openapi_values(&text, operation)
        } else {
            Ok(text)
        }
    });
    match result {
        Ok(text) => {
            d.text = text;
            d.dirty = true;
            d.diagnostics.clear();
            d.clear_preview();
            validate_editor_json(d);
        }
        Err(error) => {
            d.diagnostics = vec![error];
            d.result_tab = ResultTab::Diagnostics;
        }
    }
}

fn operation_matches_query(operation: &SpecOperation, query: &str) -> bool {
    query.is_empty()
        || operation.path.to_ascii_lowercase().contains(query)
        || operation.summary.to_ascii_lowercase().contains(query)
        || operation.id.to_ascii_lowercase().contains(query)
        || operation
            .tags
            .iter()
            .any(|tag| tag.to_ascii_lowercase().contains(query))
}

fn openapi_docs_url(spec: &ParsedSpec) -> Option<&str> {
    spec.raw
        .get("x-swagger-ui-url")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            spec.raw
                .pointer("/externalDocs/url")
                .and_then(serde_json::Value::as_str)
        })
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
}

fn method_color(method: Method) -> egui::Color32 {
    match method {
        Method::Get => egui::Color32::from_rgb(0x61, 0xAF, 0xEF),
        Method::Post => egui::Color32::from_rgb(0x47, 0xC9, 0x82),
        Method::Put | Method::Patch => egui::Color32::from_rgb(0xE5, 0xC0, 0x7B),
        Method::Delete => egui::Color32::from_rgb(0xE0, 0x6C, 0x75),
        _ => egui::Color32::from_rgb(0xC6, 0x78, 0xDD),
    }
}

fn advisor_sidebar(ui: &mut egui::Ui, d: &mut V1EditorState, bridge: &Bridge) {
    let ready =
        !d.advisor_config.endpoint.trim().is_empty() && !d.advisor_config.model.trim().is_empty();
    let active_file = d
        .file
        .as_deref()
        .and_then(|file| {
            d.root
                .as_deref()
                .and_then(|root| file.strip_prefix(root).ok())
                .or(Some(file))
        })
        .map(|file| file.display().to_string())
        .unwrap_or_else(|| "Unsaved request".to_string());
    ui.horizontal_wrapped(|ui| {
        ui.strong("Context");
        ui.monospace(&active_file)
            .on_hover_text("The currently open request is always included and redacted.");
    });
    ui.add_space(8.0);
    egui::CollapsingHeader::new("Connection")
        .default_open(!ready)
        .show(ui, |ui| {
            ui.label("OpenAI-compatible base URL");
            ui.add(
                TextEdit::singleline(&mut d.advisor_config.endpoint)
                    .hint_text("http://localhost:11434/v1"),
            );
            ui.label("Model");
            ui.add(TextEdit::singleline(&mut d.advisor_config.model).hint_text("model-name"));
            ui.label("API key variable (optional)")
                .on_hover_text("Only the variable name is saved locally; never the key value.");
            ui.add(
                TextEdit::singleline(&mut d.advisor_config.api_key_env).hint_text("OPENAI_API_KEY"),
            );
            if ui.small_button("Save connection").clicked() {
                match d
                    .root
                    .as_deref()
                    .ok_or_else(|| "no project root".to_string())
                    .and_then(|root| crate::advisor::save(root, &d.advisor_config))
                {
                    Ok(()) => d.advisor_error = None,
                    Err(error) => d.advisor_error = Some(error),
                }
            }
        });
    ui.add_space(8.0);
    ui.label("Question");
    ui.add(
        TextEdit::multiline(&mut d.advisor_question)
            .desired_rows(4)
            .hint_text("What should the advisor review?"),
    );
    ui.add_enabled_ui(d.last_response.is_some(), |ui| {
        ui.checkbox(&mut d.advisor_include_response, "Include last response");
    });
    ui.label("Redacted context").on_hover_text(
        "The current request is always included. Sensitive values are redacted before sending.",
    );
    ui.add_space(6.0);
    let asking = d.active_advisor.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(ready && !asking, egui::Button::new("Ask advisor"))
            .clicked()
        {
            start_advisor(d, bridge);
        }
        if asking {
            ui.spinner();
        }
        if d.advisor_answer.is_some() && ui.small_button("Copy").clicked() {
            ui.ctx()
                .copy_text(d.advisor_answer.clone().unwrap_or_default());
        }
    });
    if let Some(error) = &d.advisor_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(answer) = &d.advisor_answer {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("advisor-answer")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(egui::Label::new(answer).wrap().selectable(true));
            });
    }
}

fn start_advisor(d: &mut V1EditorState, bridge: &Bridge) {
    let Some(root) = d.root.clone() else {
        d.advisor_error = Some("no project root".to_string());
        return;
    };
    if let Err(error) = crate::advisor::save(&root, &d.advisor_config) {
        d.advisor_error = Some(error);
        return;
    }
    let context = match advisor_context(d) {
        Ok(context) => context,
        Err(error) => {
            d.advisor_error = Some(error);
            return;
        }
    };
    d.next_advisor_id += 1;
    let advisor_id = d.next_advisor_id;
    d.active_advisor = Some(advisor_id);
    d.advisor_answer = None;
    d.advisor_error = None;
    if let Err(error) = bridge.send(Cmd::AskAdvisor {
        advisor_id,
        root,
        config: d.advisor_config.clone(),
        question: d.advisor_question.clone(),
        context,
    }) {
        d.active_advisor = None;
        d.advisor_error = Some(error);
    }
}

fn advisor_context(d: &V1EditorState) -> Result<String, String> {
    let mut request: serde_json::Value =
        serde_json::from_str(&d.text).map_err(|error| format!("invalid request JSON: {error}"))?;
    redact_sensitive_json(&mut request);
    let file = d
        .file
        .as_deref()
        .and_then(|file| {
            d.root
                .as_deref()
                .and_then(|root| file.strip_prefix(root).ok())
                .or(Some(file))
        })
        .map(|file| file.display().to_string())
        .unwrap_or_else(|| "unsaved request".to_string());
    let mut sections = vec![format!(
        "Workspace context:\nroot={}\nactive_file={}\nThe active file is authoritative; use the surrounding project files below only as supporting context.",
        d.root.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "unknown".into()),
        file,
    ), format!(
        "Current file: {file}\nRequest document:\n{}",
        serde_json::to_string_pretty(&request).map_err(|error| error.to_string())?
    )];

    // Keep the advisor useful without asking the user to curate a file list.
    // These are the files that define how this request behaves at runtime.
    for (label, value) in [
        ("Assertions", serde_json::to_value(&d.assertions).ok()),
        ("Hooks", serde_json::to_value(&d.hooks).ok()),
        ("Auth", serde_json::to_value(&d.project_auth).ok()),
    ] {
        if let Some(mut value) = value {
            redact_sensitive_json(&mut value);
            sections.push(format!(
                "Active file {label} sidecar:\n{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            ));
        }
    }

    if let Some(root) = &d.root {
        for relative in ["project.json", "forge.json"] {
            let path = root.join(relative);
            if let Ok(text) = std::fs::read_to_string(&path) {
                sections.push(format!(
                    "Project metadata ({relative}):\n{}",
                    truncate_text(&text, 8_000)
                ));
            }
        }
        if let Some(source) = &d.openapi_source {
            if let Ok(text) = std::fs::read_to_string(source) {
                sections.push(format!(
                    "OpenAPI source ({}):\n{}",
                    source.strip_prefix(root).unwrap_or(source).display(),
                    truncate_text(&text, 16_000)
                ));
            }
        }
        let mut related = Vec::new();
        let current = d.file.as_deref();
        if let Some(entries) = d
            .file
            .as_deref()
            .and_then(Path::parent)
            .and_then(|dir| std::fs::read_dir(dir).ok())
        {
            for path in entries
                .flatten()
                .map(|entry| entry.path())
                .take(20)
                .filter(|path| Some(path.as_path()) != current && path.is_file())
                .filter(|path| {
                    matches!(
                        path.extension().and_then(|e| e.to_str()),
                        Some("json" | "yaml" | "yml" | "js")
                    )
                })
            {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                related.push(format!(
                    "{}:\n{}",
                    path.strip_prefix(root).unwrap_or(&path).display(),
                    truncate_text(&text, 6_000)
                ));
            }
        }
        if !related.is_empty() {
            sections.push(format!(
                "Related files in the active folder:\n{}",
                related.join("\n\n")
            ));
        }
    }

    if let (Some(spec), Ok(document)) = (
        d.openapi.as_ref(),
        forge_core::reqv1::RequestDocument::parse(&d.text),
    ) {
        if let Some(operation) = spec.find_operation(document.request.method, &document.request.url)
        {
            let contract = serde_json::json!({
                "title": spec.title,
                "version": spec.version,
                "operationId": operation.id,
                "method": operation.method.as_str(),
                "path": operation.path,
                "summary": operation.summary,
                "pathParameters": operation.path_params,
                "queryParameters": operation.query_params,
                "headerParameters": operation.header_params,
                "requestContentType": operation.request_content_type,
                "requestSchema": operation.request_schema,
                "responses": operation.responses.iter().map(|response| serde_json::json!({
                    "status": response.status,
                    "contentType": response.content_type,
                    "schema": response.schema,
                })).collect::<Vec<_>>(),
            });
            sections.push(format!(
                "Matching OpenAPI operation:\n{}",
                serde_json::to_string_pretty(&contract).map_err(|error| error.to_string())?
            ));
        }
    }

    if d.advisor_include_response {
        if let Some(response) = &d.last_response {
            let headers = response
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        if sensitive_name(name) {
                            "***".to_string()
                        } else {
                            value.clone()
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let body = if let Some(mut json) = response.json() {
                redact_sensitive_json(&mut json);
                serde_json::to_string_pretty(&json).unwrap_or_default()
            } else {
                response.text().into_owned()
            };
            sections.push(format!(
                "Last response:\nstatus={}\ntimeMs={}\nheaders={}\nbody={} ",
                response.status,
                response.time_ms,
                serde_json::to_string(&headers).unwrap_or_default(),
                truncate_text(&body, 12_000)
            ));
        }
    }
    Ok(truncate_text(&sections.join("\n\n"), 48_000))
}

fn redact_sensitive_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let masks_header = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(sensitive_name);
            for (name, value) in object {
                if sensitive_name(name) || (masks_header && name == "value") {
                    *value = serde_json::Value::String("***".to_string());
                } else {
                    redact_sensitive_json(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_sensitive_json(value);
            }
        }
        _ => {}
    }
}

fn sensitive_name(name: &str) -> bool {
    let name = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "apikey",
        "password",
        "secret",
        "token",
        "cookie",
    ]
    .iter()
    .any(|sensitive| name.contains(sensitive))
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n… truncated")
    } else {
        truncated
    }
}

fn palette(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    bridge: &Bridge,
    insert: &mut Option<PendingInsert>,
) {
    let assets = d
        .index
        .as_ref()
        .map(|index| index.assets.clone())
        .unwrap_or_default();
    catalog_filters(ui, d);
    let query = d.catalog_query.trim().to_ascii_lowercase();
    let intent_filter = d.catalog_intent.clone();
    let intent = intent_filter.as_deref();
    let has_selection = match d.catalog_view {
        CatalogView::Builtins => d.selected_builtin.is_some(),
        CatalogView::Project => d.selected_project.is_some(),
    };
    ui.add_space(6.0);
    ui.separator();
    let list_height = if has_selection {
        (ui.available_height() * 0.4).clamp(170.0, 320.0)
    } else {
        (ui.available_height() - 12.0).max(180.0)
    };
    egui::ScrollArea::vertical()
        .id_salt("catalog-results")
        .max_height(list_height)
        .auto_shrink([false, false])
        .show(ui, |ui| match d.catalog_view {
            CatalogView::Builtins => builtin_list(ui, d, &query, intent),
            CatalogView::Project => project_asset_list(ui, d, &assets, &query, intent, insert),
        });

    if !has_selection {
        return;
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("CONFIGURE").small().strong().weak());
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("catalog-detail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| match d.catalog_view {
                    CatalogView::Builtins => {
                        let definition = d
                            .selected_builtin
                            .as_deref()
                            .and_then(find_builtin)
                            .copied();
                        if let Some(definition) = definition {
                            builtin_form(ui, d, bridge, insert, definition);
                        } else {
                            ui.weak("Select a built-in to configure it.");
                        }
                    }
                    CatalogView::Project => {
                        let asset = d.selected_project.as_ref().and_then(|selected| {
                            assets.iter().find(|asset| &asset.rel_path == selected)
                        });
                        if let Some(asset) = asset {
                            project_asset_form(ui, d, bridge, asset, insert);
                        } else {
                            ui.weak("Select a project asset to configure it.");
                        }
                    }
                });
        });
}

fn catalog_filters(ui: &mut egui::Ui, d: &mut V1EditorState) {
    ui.add_sized(
        [ui.available_width(), 32.0],
        TextEdit::singleline(&mut d.catalog_query)
            .hint_text(format!("{}  Search catalog", icons::SEARCH)),
    )
    .on_hover_text("Search reusable behavior by title, intent or description");
    ui.add_space(6.0);
    let width = ((ui.available_width() - 8.0) / 2.0).max(100.0);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("catalog-source")
            .width(width)
            .selected_text(match d.catalog_view {
                CatalogView::Builtins => "Built-ins",
                CatalogView::Project => "Project assets",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.catalog_view, CatalogView::Builtins, "Built-ins");
                ui.selectable_value(&mut d.catalog_view, CatalogView::Project, "Project assets");
            })
            .response
            .on_hover_text("Catalog source");
        egui::ComboBox::from_id_salt("catalog-intent")
            .width(width)
            .selected_text(d.catalog_intent.as_deref().unwrap_or("All intents"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.catalog_intent, None, "All intents");
                for intent in CATALOG_INTENTS {
                    let label = intent_label(intent);
                    ui.selectable_value(&mut d.catalog_intent, Some(label.to_string()), label);
                }
            })
            .response
            .on_hover_text("Filter by intent");
    });
}

const CATALOG_INTENTS: [BuiltinIntent; 5] = [
    BuiltinIntent::Validate,
    BuiltinIntent::Prepare,
    BuiltinIntent::Capture,
    BuiltinIntent::Generate,
    BuiltinIntent::Simulate,
];

fn builtin_matches(definition: &BuiltinDefinition, query: &str, intent: Option<&str>) -> bool {
    intent.is_none_or(|intent| intent == intent_label(definition.intent))
        && (query.is_empty()
            || definition.title.to_ascii_lowercase().contains(query)
            || definition.description.to_ascii_lowercase().contains(query)
            || definition.name.to_ascii_lowercase().contains(query)
            || intent_label(definition.intent)
                .to_ascii_lowercase()
                .contains(query))
}

fn project_asset_matches(asset: &AssetEntry, query: &str, intent: Option<&str>) -> bool {
    let asset_intent = asset
        .metadata
        .as_ref()
        .map(|metadata| intent_label(metadata.intent))
        .or_else(|| asset_intent(asset.kind));
    intent.is_none_or(|filter| asset_intent == Some(filter))
        && (query.is_empty()
            || asset.rel_path.to_ascii_lowercase().contains(query)
            || asset
                .alias
                .as_deref()
                .is_some_and(|alias| alias.to_ascii_lowercase().contains(query))
            || asset.metadata.as_ref().is_some_and(|metadata| {
                metadata.title.to_ascii_lowercase().contains(query)
                    || metadata.description.to_ascii_lowercase().contains(query)
            }))
}

fn catalog_section_header(ui: &mut egui::Ui, label: &str, count: usize) {
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(label.to_uppercase()).small().strong().weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(count.to_string()).small().weak());
        });
    });
}

fn builtin_list(ui: &mut egui::Ui, d: &mut V1EditorState, query: &str, intent: Option<&str>) {
    let mut found = false;
    for group in CATALOG_INTENTS {
        let count = builtin_catalog()
            .iter()
            .filter(|definition| {
                definition.intent == group && builtin_matches(definition, query, intent)
            })
            .count();
        if count == 0 {
            continue;
        }
        found = true;
        catalog_section_header(ui, intent_label(group), count);
        for definition in builtin_catalog().iter().filter(|definition| {
            definition.intent == group && builtin_matches(definition, query, intent)
        }) {
            let selected = d.selected_builtin.as_deref() == Some(definition.name);
            let row_width = ui.available_width();
            let show_target = row_width >= 260.0;
            let title_width = if show_target {
                (row_width - 106.0).max(100.0)
            } else {
                row_width
            };
            let response = ui
                .horizontal(|ui| {
                    let response = ui
                        .allocate_ui_with_layout(
                            egui::vec2(title_width, ui.spacing().interact_size.y),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.selectable_label(selected, definition.title),
                        )
                        .inner;
                    if show_target {
                        ui.label(
                            RichText::new(target_label(definition.target))
                                .small()
                                .weak(),
                        );
                    }
                    response
                })
                .inner
                .on_hover_text(definition.description);
            if response.clicked() {
                select_builtin(d, definition);
            }
        }
    }
    if !found {
        ui.weak("No matching built-ins.");
    }
}

fn project_asset_group(asset: &AssetEntry) -> &'static str {
    asset
        .metadata
        .as_ref()
        .map(|metadata| intent_label(metadata.intent))
        .or_else(|| asset_intent(asset.kind))
        .unwrap_or(match asset.kind {
            AssetKind::Data => "Data",
            _ => "Other",
        })
}

fn project_asset_list(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    assets: &[AssetEntry],
    query: &str,
    intent: Option<&str>,
    insert: &mut Option<PendingInsert>,
) {
    const GROUPS: [&str; 7] = [
        "Validate", "Prepare", "Capture", "Generate", "Simulate", "Data", "Other",
    ];
    let selected = d.selected_project.clone();
    let mut select_project = None;
    let mut found = false;
    for group in GROUPS {
        let count = assets
            .iter()
            .filter(|asset| {
                project_asset_group(asset) == group && project_asset_matches(asset, query, intent)
            })
            .count();
        if count == 0 {
            continue;
        }
        found = true;
        catalog_section_header(ui, group, count);
        for asset in assets.iter().filter(|asset| {
            project_asset_group(asset) == group && project_asset_matches(asset, query, intent)
        }) {
            palette_row(
                ui,
                asset,
                selected.as_deref() == Some(&asset.rel_path),
                &mut d.expanded,
                insert,
                &mut select_project,
            );
        }
    }
    if !found {
        ui.weak("No matching project assets.");
    }
    if let Some(rel_path) = select_project {
        if let Some(asset) = assets.iter().find(|asset| asset.rel_path == rel_path) {
            select_project_asset(d, asset);
        }
    }
}

fn builtin_form(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    bridge: &Bridge,
    insert: &mut Option<PendingInsert>,
    definition: BuiltinDefinition,
) {
    let heading = ui.label(RichText::new(definition.title).strong());
    if std::mem::take(&mut d.scroll_to_catalog_form) {
        heading.scroll_to_me(Some(egui::Align::TOP));
    }
    ui.weak(format!(
        "{} · {}",
        intent_label(definition.intent),
        target_label(definition.target)
    ));
    ui.weak(definition.description);
    let mut form_changed = false;
    for parameter in definition.parameters {
        form_changed |= parameter_form(ui, d, &ParameterDefinition::builtin(parameter));
    }
    if form_changed {
        d.clear_preview();
    }
    if let Some(error) = &d.catalog_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(notice) = &d.catalog_notice {
        ui.weak(notice);
    }

    let preview_phase = match definition.target {
        BuiltinTarget::Pipeline(phase) => Some(phase),
        BuiltinTarget::Binding => None,
    };
    let has_preview_context = preview_phase.is_some_and(|phase| {
        phase == forge_core::reqv1::model::PipelinePhase::BeforeRequest || d.last_response.is_some()
    });
    let preview_sources_supported = preview_supports_sources(&d.catalog_inputs);
    ui.horizontal(|ui| {
        let action = if d.editing_assertion.is_some() {
            "Save assertion"
        } else if d.editing_hook.is_some() {
            "Save hook"
        } else {
            "Insert configured reference"
        };
        if ui.button(action).clicked() {
            match builtin_snippet(&definition, &d.catalog_inputs) {
                Ok(snippet) => {
                    d.catalog_error = None;
                    *insert = Some(PendingInsert {
                        target: match definition.target {
                            BuiltinTarget::Binding => InsertTarget::Binding,
                            BuiltinTarget::Pipeline(_)
                                if definition.intent == BuiltinIntent::Validate =>
                            {
                                InsertTarget::Assertion
                            }
                            BuiltinTarget::Pipeline(_) => InsertTarget::Pipeline,
                        },
                        suggested_name: definition.name.to_string(),
                        snippet,
                    });
                }
                Err(error) => d.catalog_error = Some(error),
            }
        }
        if ui
            .add_enabled(
                has_preview_context && preview_sources_supported && !d.preview_in_flight,
                egui::Button::new("Preview"),
            )
            .on_hover_text("Evaluate locally; never sends an HTTP request")
            .clicked()
        {
            preview_now(d, bridge, &definition);
        }
        if d.preview_in_flight {
            ui.spinner();
        }
    });
    if preview_phase.is_none() {
        ui.weak("Binding generators have no request/response preview.");
    } else if !preview_sources_supported {
        ui.weak("Preview cannot resolve these inputs; use a matrix run or runtime sequence.");
    } else if !has_preview_context {
        ui.weak("Run the request once to preview an afterResponse asset.");
    }
    preview_pane(ui, d);
    ui.collapsing("Example", |ui| {
        ui.monospace(definition.example);
    });
}

fn preview_supports_sources(inputs: &BTreeMap<String, ParameterInput>) -> bool {
    inputs.values().all(|input| {
        !matches!(
            input.source,
            ParameterSource::Matrix | ParameterSource::Runtime
        )
    })
}

fn intent_label(intent: BuiltinIntent) -> &'static str {
    match intent {
        BuiltinIntent::Validate => "Validate",
        BuiltinIntent::Prepare => "Prepare",
        BuiltinIntent::Capture => "Capture",
        BuiltinIntent::Generate => "Generate",
        BuiltinIntent::Simulate => "Simulate",
    }
}

fn target_label(target: BuiltinTarget) -> &'static str {
    match target {
        BuiltinTarget::Binding => "Binding",
        BuiltinTarget::Pipeline(forge_core::reqv1::model::PipelinePhase::BeforeRequest) => {
            "beforeRequest"
        }
        BuiltinTarget::Pipeline(forge_core::reqv1::model::PipelinePhase::AfterResponse) => {
            "afterResponse"
        }
        BuiltinTarget::Pipeline(forge_core::reqv1::model::PipelinePhase::OnError) => "onError",
        BuiltinTarget::Pipeline(forge_core::reqv1::model::PipelinePhase::Finally) => "finally",
    }
}

fn asset_intent(kind: AssetKind) -> Option<&'static str> {
    match kind {
        AssetKind::Assertion => Some("Validate"),
        AssetKind::Hook => Some("Prepare"),
        AssetKind::Extractor => Some("Capture"),
        AssetKind::Generator => Some("Generate"),
        AssetKind::Mock => Some("Simulate"),
        AssetKind::Data | AssetKind::Executable => None,
    }
}

fn select_builtin(d: &mut V1EditorState, definition: &BuiltinDefinition) {
    d.catalog_view = CatalogView::Builtins;
    d.selected_builtin = Some(definition.name.to_string());
    d.selected_project = None;
    d.editing_assertion = None;
    d.editing_hook = None;
    d.scroll_to_catalog_form = true;
    d.catalog_error = None;
    d.catalog_notice = None;
    d.clear_preview();
    d.catalog_inputs.clear();
    let example = serde_json::from_str::<serde_json::Value>(definition.example)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for parameter in definition.parameters {
        let parameter = ParameterDefinition::builtin(parameter);
        let value = parameter
            .default
            .clone()
            .or_else(|| {
                parameter
                    .required
                    .then(|| example.get(&parameter.name).cloned())
                    .flatten()
            })
            .map(|value| display_parameter_value(parameter.kind, &value))
            .unwrap_or_default();
        d.catalog_inputs.insert(
            parameter.name,
            ParameterInput {
                source: ParameterSource::Literal,
                value,
            },
        );
    }
}

fn parameter_form(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    parameter: &ParameterDefinition,
) -> bool {
    let mut changed = false;
    ui.add_space(4.0);
    ui.label(if parameter.required {
        RichText::new(format!("{} *", parameter.label))
    } else {
        RichText::new(&parameter.label)
    });

    let current_source = d
        .catalog_inputs
        .get(&parameter.name)
        .map(|input| input.source)
        .unwrap_or_default();
    let suggestions = parameter_suggestions(d, current_source);
    let input = d.catalog_inputs.entry(parameter.name.clone()).or_default();

    let previous_source = input.source;
    let previous_value = input.value.clone();
    let source_response =
        egui::ComboBox::from_id_salt(("catalog-source", definition_id(parameter)))
            .selected_text(input.source.label())
            .show_ui(ui, |ui| {
                for &source in parameter_sources(parameter.kind) {
                    ui.selectable_value(&mut input.source, source, source.label());
                }
            });
    changed |= source_response.response.changed();
    if input.source != previous_source {
        input.value.clear();
        changed = true;
    }

    if input.source == ParameterSource::Literal && !parameter.options.is_empty() {
        let option_response =
            egui::ComboBox::from_id_salt(("catalog-option", definition_id(parameter)))
                .selected_text(if input.value.is_empty() {
                    "(select)"
                } else {
                    &input.value
                })
                .show_ui(ui, |ui| {
                    for option in &parameter.options {
                        ui.selectable_value(&mut input.value, option.clone(), option);
                    }
                });
        changed |= option_response.response.changed();
    } else {
        changed |= ui
            .add(TextEdit::singleline(&mut input.value).hint_text(
                if input.source == ParameterSource::Literal {
                    parameter.example.as_str()
                } else {
                    "namespace path"
                },
            ))
            .changed();
        if !suggestions.is_empty() {
            egui::ComboBox::from_id_salt(("catalog-suggestion", definition_id(parameter)))
                .selected_text("Suggestions")
                .show_ui(ui, |ui| {
                    for suggestion in &suggestions {
                        if ui.selectable_label(false, suggestion).clicked() {
                            input.value.clone_from(suggestion);
                            changed = true;
                            ui.close();
                        }
                    }
                });
        }
    }
    if input.source == ParameterSource::Runtime {
        ui.weak("Last run output; available only to a later sequence step.");
    }
    changed || input.value != previous_value
}

fn definition_id(parameter: &ParameterDefinition) -> &str {
    &parameter.name
}

fn parameter_suggestions(d: &V1EditorState, source: ParameterSource) -> Vec<String> {
    let document = forge_core::reqv1::RequestDocument::parse(&d.text).ok();
    let mut suggestions = Vec::new();
    match source {
        ParameterSource::Binding => {
            if let Some(document) = &document {
                binding_paths(&document.bindings, &mut suggestions);
            }
        }
        ParameterSource::Matrix => {
            if let Some(document) = &document {
                binding_paths(&document.matrix, &mut suggestions);
            }
        }
        ParameterSource::Environment => {
            if let (Some(root), Some(file)) = (&d.root, &d.file) {
                if let Ok(environment) =
                    forge_core::reqv1::load_request_environment(root, file, d.env_name.as_deref())
                {
                    collect_value_paths(&environment, "", &mut suggestions);
                }
            }
        }
        ParameterSource::Runtime => {
            if let Some(result) = selected_result(d) {
                for (name, value) in &result.runtime {
                    suggestions.push(name.clone());
                    collect_value_paths(value, name, &mut suggestions);
                }
            }
        }
        ParameterSource::Secret => {
            if let Some(root) = &d.root {
                suggestions.extend(forge_core::reqv1::load_file_secrets(root).into_keys());
            }
        }
        ParameterSource::Literal => {}
    }
    suggestions.sort();
    suggestions.dedup();
    suggestions
}

fn binding_paths(bindings: &BTreeMap<String, forge_core::reqv1::Binding>, paths: &mut Vec<String>) {
    for (name, binding) in bindings {
        paths.push(name.clone());
        if let forge_core::reqv1::Binding::Value(value) = binding {
            collect_value_paths(&value.value, name, paths);
        }
    }
}

fn collect_value_paths(value: &serde_json::Value, prefix: &str, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                paths.push(path.clone());
                collect_value_paths(value, &path, paths);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                let path = format!("{prefix}.{index}");
                paths.push(path.clone());
                collect_value_paths(value, &path, paths);
            }
        }
        _ => {}
    }
}

fn builtin_snippet(
    definition: &BuiltinDefinition,
    inputs: &BTreeMap<String, ParameterInput>,
) -> Result<String, String> {
    let with = builtin_with(definition, inputs)?;
    let reference = definition.reference;
    let with = serde_json::Value::Object(with);
    let snippet = match definition.target {
        BuiltinTarget::Binding => serde_json::json!({ "use": reference, "with": with }),
        BuiltinTarget::Pipeline(phase) => serde_json::json!({
            "phase": phase,
            "use": reference,
            "with": with,
        }),
    };
    serde_json::to_string_pretty(&snippet).map_err(|error| error.to_string())
}

fn builtin_with(
    definition: &BuiltinDefinition,
    inputs: &BTreeMap<String, ParameterInput>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let parameters = definition
        .parameters
        .iter()
        .map(ParameterDefinition::builtin)
        .collect::<Vec<_>>();
    configured_with(&parameters, inputs)
}

fn configured_with(
    parameters: &[ParameterDefinition],
    inputs: &BTreeMap<String, ParameterInput>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut with = serde_json::Map::new();
    for parameter in parameters {
        let input = inputs.get(&parameter.name).cloned().unwrap_or_default();
        let value = if input.source == ParameterSource::Literal {
            literal_value(parameter, &input.value)?
        } else {
            sourced_value(input.source, &input.value)
        };
        if let Some(value) = value {
            with.insert(parameter.name.clone(), value);
        } else if parameter.required {
            return Err(format!("{} is required", parameter.label));
        }
    }
    Ok(with)
}

fn preview_now(d: &mut V1EditorState, bridge: &Bridge, definition: &BuiltinDefinition) {
    let BuiltinTarget::Pipeline(phase) = definition.target else {
        return;
    };
    if !preview_supports_sources(&d.catalog_inputs) {
        d.preview_error = Some(
            "Preview cannot resolve these inputs; use a matrix run or runtime sequence.".into(),
        );
        return;
    }
    let Some(root) = d.root.clone() else {
        d.preview_error = Some("No project root.".to_string());
        return;
    };
    let with = match builtin_with(definition, &d.catalog_inputs) {
        Ok(with) => with,
        Err(error) => {
            d.preview_error = Some(error);
            return;
        }
    };
    let file = d
        .file
        .clone()
        .unwrap_or_else(|| root.join("__unsaved__.request.json"));
    let preview_id = d.next_preview_id;
    d.next_preview_id += 1;
    d.active_preview = Some(preview_id);
    d.preview_in_flight = true;
    d.preview = None;
    d.preview_error = None;
    if let Err(error) = bridge.send(Cmd::PreviewV1Asset {
        preview_id,
        root,
        file,
        text: d.text.clone(),
        env_name: d.env_name.clone(),
        phase,
        uses: definition.reference.to_string(),
        with,
        response: d.last_response.clone(),
        allow_project_code: d.allow_project_code,
    }) {
        d.active_preview = None;
        d.preview_in_flight = false;
        d.preview_error = Some(error);
    }
}

fn preview_pane(ui: &mut egui::Ui, d: &V1EditorState) {
    if let Some(error) = &d.preview_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let Some(preview) = &d.preview else {
        return;
    };

    ui.group(|ui| {
        ui.label(RichText::new("Preview").strong());
        if let (Some(before), Some(after)) = (&preview.request_before, &preview.request_after) {
            request_diff(ui, before, after);
        }
        for assertion in &preview.assertions {
            let (mark, color) = if assertion.passed {
                ("✓", egui::Color32::from_rgb(0x49, 0x9C, 0x54))
            } else {
                ("✗", ui.visuals().error_fg_color)
            };
            ui.colored_label(color, format!("{mark} {}", assertion.message));
            if !assertion.passed {
                if let Some(expected) = &assertion.expected {
                    ui.weak(format!("expected: {expected}"));
                }
                if let Some(actual) = &assertion.actual {
                    ui.weak(format!("actual: {actual}"));
                }
            }
        }
        if !preview.runtime_writes.is_empty() {
            ui.label(RichText::new("Runtime writes").strong());
            for (name, value) in &preview.runtime_writes {
                ui.monospace(format!("{name} = {value}"));
            }
        }
        if !preview.logs.is_empty() {
            ui.label(RichText::new("Logs (secrets masked)").strong());
            for line in &preview.logs {
                ui.monospace(line);
            }
        }
        for diagnostic in &preview.diagnostics {
            let color = if diagnostic.severity == forge_core::reqv1::Severity::Error {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().warn_fg_color
            };
            ui.colored_label(
                color,
                format!("[{}] {}", diagnostic.code, diagnostic.message),
            );
        }
        if preview.request_before.is_none()
            && preview.assertions.is_empty()
            && preview.runtime_writes.is_empty()
            && preview.logs.is_empty()
            && preview.diagnostics.is_empty()
        {
            ui.weak("No observable changes.");
        }
    });
}

fn request_diff(
    ui: &mut egui::Ui,
    before: &forge_core::reqv1::runner::CatalogRequestView,
    after: &forge_core::reqv1::runner::CatalogRequestView,
) {
    let mut changed = false;
    if before.url != after.url {
        changed = true;
        ui.label(RichText::new("URL").strong());
        ui.weak(format!("- {}", before.url));
        ui.label(format!("+ {}", after.url));
    }

    // ponytail: duplicate request headers collapse by case-insensitive name;
    // use a multiset diff if repeated header editing becomes a real use case.
    let header_map = |headers: &[(String, String)]| {
        headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), (name.clone(), value.clone())))
            .collect::<BTreeMap<_, _>>()
    };
    let before_headers = header_map(&before.headers);
    let after_headers = header_map(&after.headers);
    let header_names = before_headers
        .keys()
        .chain(after_headers.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in header_names {
        let before_header = before_headers.get(&key);
        let after_header = after_headers.get(&key);
        if before_header == after_header {
            continue;
        }
        changed = true;
        match (before_header, after_header) {
            (Some((name, value)), None) => {
                ui.weak(format!("- {name}: {value}"));
            }
            (None, Some((name, value))) => {
                ui.label(format!("+ {name}: {value}"));
            }
            (Some((name, before_value)), Some((_, after_value))) => {
                ui.label(format!("~ {name}: {before_value} → {after_value}"));
            }
            (None, None) => {}
        }
    }
    if !changed {
        ui.weak("No request changes.");
    }
}

fn literal_value(
    parameter: &ParameterDefinition,
    input: &str,
) -> Result<Option<serde_json::Value>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    if !parameter.options.is_empty() && !parameter.options.iter().any(|option| option == input) {
        return Err(format!(
            "{} must be one of {}",
            parameter.label,
            parameter.options.join(", ")
        ));
    }
    let value = match parameter.kind {
        BuiltinParameterKind::String => serde_json::Value::String(input.to_string()),
        BuiltinParameterKind::Integer => serde_json::Value::Number(
            input
                .parse::<u64>()
                .map_err(|_| format!("{} must be a non-negative integer", parameter.label))?
                .into(),
        ),
        BuiltinParameterKind::Boolean => serde_json::Value::Bool(
            input
                .parse::<bool>()
                .map_err(|_| format!("{} must be true or false", parameter.label))?,
        ),
        BuiltinParameterKind::Json => serde_json::from_str(input)
            .map_err(|error| format!("{} is invalid JSON: {error}", parameter.label))?,
    };
    Ok(Some(value))
}

fn display_parameter_value(kind: BuiltinParameterKind, value: &serde_json::Value) -> String {
    match kind {
        BuiltinParameterKind::String => value.as_str().unwrap_or_default().to_string(),
        _ => value.to_string(),
    }
}

fn parameter_input_from_value(
    kind: BuiltinParameterKind,
    value: &serde_json::Value,
) -> ParameterInput {
    if let Some((namespace, path)) = value
        .as_str()
        .and_then(|value| value.strip_prefix("${"))
        .and_then(|value| value.strip_suffix('}'))
        .and_then(|value| value.split_once('.'))
    {
        let source = match namespace {
            "bindings" => Some(ParameterSource::Binding),
            "env" => Some(ParameterSource::Environment),
            "runtime" => Some(ParameterSource::Runtime),
            "matrix" => Some(ParameterSource::Matrix),
            "secret" => Some(ParameterSource::Secret),
            _ => None,
        };
        if let Some(source) = source.filter(|_| !path.is_empty()) {
            return ParameterInput {
                source,
                value: path.to_string(),
            };
        }
    }
    ParameterInput {
        source: ParameterSource::Literal,
        value: display_parameter_value(kind, value),
    }
}

fn select_project_asset(d: &mut V1EditorState, asset: &AssetEntry) {
    let Some(metadata) = &asset.metadata else {
        return;
    };
    d.catalog_view = CatalogView::Project;
    d.selected_builtin = None;
    d.selected_project = Some(asset.rel_path.clone());
    d.editing_assertion = None;
    d.editing_hook = None;
    d.scroll_to_catalog_form = true;
    d.catalog_inputs.clear();
    d.catalog_error = None;
    d.catalog_notice = None;
    d.clear_preview();
    let example = metadata.example.as_object().cloned().unwrap_or_default();
    for parameter in &metadata.parameters {
        let parameter = ParameterDefinition::project(parameter);
        let value = parameter
            .default
            .clone()
            .or_else(|| {
                parameter
                    .required
                    .then(|| example.get(&parameter.name).cloned())
                    .flatten()
            })
            .map(|value| display_parameter_value(parameter.kind, &value))
            .unwrap_or_default();
        d.catalog_inputs.insert(
            parameter.name,
            ParameterInput {
                source: ParameterSource::Literal,
                value,
            },
        );
    }
}

fn project_asset_form(
    ui: &mut egui::Ui,
    d: &mut V1EditorState,
    bridge: &Bridge,
    asset: &AssetEntry,
    insert: &mut Option<PendingInsert>,
) {
    let Some(metadata) = &asset.metadata else {
        return;
    };
    let base_ref = asset
        .alias
        .clone()
        .or_else(|| asset.prefix_ref.clone())
        .unwrap_or_else(|| asset.rel_path.clone());
    let heading = ui.label(RichText::new(&metadata.title).strong());
    if std::mem::take(&mut d.scroll_to_catalog_form) {
        heading.scroll_to_me(Some(egui::Align::TOP));
    }
    ui.weak(format!(
        "{} · {}",
        intent_label(metadata.intent),
        project_target_label(asset, metadata)
    ));
    if !metadata.description.is_empty() {
        ui.weak(&metadata.description);
    }
    let parameters = metadata
        .parameters
        .iter()
        .map(ParameterDefinition::project)
        .collect::<Vec<_>>();
    let mut changed = false;
    for parameter in &parameters {
        changed |= parameter_form(ui, d, parameter);
    }
    if changed {
        d.clear_preview();
    }
    if let Some(error) = &d.catalog_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(notice) = &d.catalog_notice {
        ui.weak(notice);
    }
    let phase = project_phase(asset, metadata);
    ui.horizontal(|ui| {
        let action = if d.editing_assertion.is_some() {
            "Save assertion"
        } else if d.editing_hook.is_some() {
            "Save hook"
        } else {
            "Insert configured reference"
        };
        if ui.button(action).clicked() {
            match project_snippet(asset, &base_ref, metadata, &parameters, &d.catalog_inputs) {
                Ok(snippet) => {
                    d.catalog_error = None;
                    *insert = Some(PendingInsert {
                        target: match asset.kind {
                            AssetKind::Generator => InsertTarget::Binding,
                            AssetKind::Assertion => InsertTarget::Assertion,
                            AssetKind::Mock => InsertTarget::Mock,
                            _ => InsertTarget::Pipeline,
                        },
                        suggested_name: suggested_name(&base_ref),
                        snippet,
                    });
                }
                Err(error) => d.catalog_error = Some(error),
            }
        }
        let can_preview = phase.is_some_and(|phase| {
            phase == forge_core::reqv1::model::PipelinePhase::BeforeRequest
                || d.last_response.is_some()
        }) && preview_supports_sources(&d.catalog_inputs)
            && !d.preview_in_flight;
        if ui
            .add_enabled(can_preview, egui::Button::new("Preview"))
            .clicked()
        {
            match configured_with(&parameters, &d.catalog_inputs) {
                Ok(with) => preview_project_now(d, bridge, phase.unwrap(), base_ref.clone(), with),
                Err(error) => d.catalog_error = Some(error),
            }
        }
        if d.preview_in_flight {
            ui.spinner();
        }
    });
    preview_pane(ui, d);
    if !metadata.example.is_null() {
        ui.collapsing("Example", |ui| {
            ui.monospace(metadata.example.to_string());
        });
    }
}

fn project_phase(
    asset: &AssetEntry,
    metadata: &ProjectAssetMetadata,
) -> Option<forge_core::reqv1::model::PipelinePhase> {
    metadata.phase.or(match asset.kind {
        AssetKind::Hook => Some(forge_core::reqv1::model::PipelinePhase::BeforeRequest),
        AssetKind::Assertion | AssetKind::Extractor => {
            Some(forge_core::reqv1::model::PipelinePhase::AfterResponse)
        }
        _ => None,
    })
}

fn project_target_label(asset: &AssetEntry, metadata: &ProjectAssetMetadata) -> &'static str {
    match asset.kind {
        AssetKind::Generator => "Binding",
        AssetKind::Mock => "Mock",
        _ => project_phase(asset, metadata)
            .map(|phase| target_label(BuiltinTarget::Pipeline(phase)))
            .unwrap_or("Executable"),
    }
}

fn project_snippet(
    asset: &AssetEntry,
    reference: &str,
    metadata: &ProjectAssetMetadata,
    parameters: &[ParameterDefinition],
    inputs: &BTreeMap<String, ParameterInput>,
) -> Result<String, String> {
    let with = serde_json::Value::Object(configured_with(parameters, inputs)?);
    let snippet = match asset.kind {
        AssetKind::Generator | AssetKind::Mock => {
            serde_json::json!({"use": reference, "with": with})
        }
        _ => {
            let phase = project_phase(asset, metadata)
                .ok_or_else(|| "asset metadata needs a pipeline phase".to_string())?;
            serde_json::json!({"phase": phase, "use": reference, "with": with})
        }
    };
    serde_json::to_string_pretty(&snippet).map_err(|error| error.to_string())
}

fn preview_project_now(
    d: &mut V1EditorState,
    bridge: &Bridge,
    phase: forge_core::reqv1::model::PipelinePhase,
    uses: String,
    with: serde_json::Map<String, serde_json::Value>,
) {
    let Some(root) = d.root.clone() else {
        return;
    };
    let file = d
        .file
        .clone()
        .unwrap_or_else(|| root.join("__unsaved__.request.json"));
    let preview_id = d.next_preview_id;
    d.next_preview_id += 1;
    d.active_preview = Some(preview_id);
    d.preview_in_flight = true;
    d.preview = None;
    d.preview_error = None;
    if let Err(error) = bridge.send(Cmd::PreviewV1Asset {
        preview_id,
        root,
        file,
        text: d.text.clone(),
        env_name: d.env_name.clone(),
        phase,
        uses,
        with,
        response: d.last_response.clone(),
        allow_project_code: d.allow_project_code,
    }) {
        d.active_preview = None;
        d.preview_in_flight = false;
        d.preview_error = Some(error);
    }
}

fn palette_row(
    ui: &mut egui::Ui,
    asset: &AssetEntry,
    selected: bool,
    expanded: &mut std::collections::HashSet<String>,
    insert: &mut Option<PendingInsert>,
    select_project: &mut Option<String>,
) {
    let base_ref = asset
        .alias
        .clone()
        .or_else(|| asset.prefix_ref.clone())
        .unwrap_or_else(|| asset.rel_path.clone());
    let browsable = asset.kind == AssetKind::Data && asset.data.is_some();

    ui.horizontal(|ui| {
        if browsable {
            let open = expanded.contains(&asset.rel_path);
            if ui.small_button(if open { "▾" } else { "▸" }).clicked() {
                if open {
                    expanded.remove(&asset.rel_path);
                } else {
                    expanded.insert(asset.rel_path.clone());
                }
            }
        } else {
            ui.add_space(16.0);
        }
        let name = asset
            .metadata
            .as_ref()
            .map(|metadata| metadata.title.as_str())
            .unwrap_or_else(|| asset.rel_path.rsplit('/').next().unwrap_or(&asset.rel_path));
        if ui
            .selectable_label(selected, name)
            .on_hover_text(&base_ref)
            .clicked()
            && asset.metadata.is_some()
        {
            *select_project = Some(asset.rel_path.clone());
        }
        let can_insert = asset.kind != AssetKind::Executable;
        if ui
            .add_enabled(can_insert, egui::Button::new("Use").small())
            .on_hover_text(if can_insert {
                "add to the request document"
            } else {
                "add metadata with a pipeline phase before inserting"
            })
            .clicked()
        {
            if let Ok(pending) = insert_for_asset(asset, &base_ref) {
                *insert = Some(pending);
            }
        }
    });

    if browsable && expanded.contains(&asset.rel_path) {
        if let Some(data) = &asset.data {
            ui.indent(&asset.rel_path, |ui| {
                json_nodes(ui, &base_ref, "", data, insert)
            });
        }
    }
}

fn json_nodes(
    ui: &mut egui::Ui,
    base_ref: &str,
    pointer: &str,
    node: &serde_json::Value,
    insert: &mut Option<PendingInsert>,
) {
    use serde_json::Value;
    let children: Vec<(String, &Value)> = match node {
        Value::Object(m) => m.iter().map(|(k, v)| (escape_ptr(k), v)).collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v))
            .collect(),
        _ => return,
    };
    for (key, value) in children {
        let ptr = format!("{pointer}/{key}");
        let full = format!("{base_ref}#{ptr}");
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let label = match value {
                Value::Object(m) => format!("{key} {{{}}}", m.len()),
                Value::Array(a) => format!("{key} [{}]", a.len()),
                other => format!("{key}: {}", short(other)),
            };
            ui.label(RichText::new(label).monospace().small());
            if ui
                .small_button("insert")
                .on_hover_text(full.clone())
                .clicked()
            {
                *insert = Some(PendingInsert {
                    target: InsertTarget::Binding,
                    suggested_name: suggested_name(&full),
                    snippet: format!("{{ \"ref\": \"{full}\" }}"),
                });
            }
        });
        if value.is_object() || value.is_array() {
            ui.indent(&ptr, |ui| json_nodes(ui, base_ref, &ptr, value, insert));
        }
    }
}

/// A ready-to-paste snippet for a whole asset: a binding for data, a pipeline
/// entry for executables (phase inferred from kind).
fn snippet_for(asset: &AssetEntry, base_ref: &str) -> String {
    match asset.kind {
        AssetKind::Data => format!("{{ \"ref\": \"{base_ref}\" }}"),
        AssetKind::Generator => format!("{{ \"use\": \"{base_ref}\", \"with\": {{}} }}"),
        AssetKind::Hook => {
            format!("{{ \"phase\": \"beforeRequest\", \"use\": \"{base_ref}\", \"with\": {{}} }}")
        }
        AssetKind::Assertion | AssetKind::Extractor => {
            format!("{{ \"phase\": \"afterResponse\", \"use\": \"{base_ref}\", \"with\": {{}} }}")
        }
        AssetKind::Mock => format!("{{ \"use\": \"{base_ref}\", \"with\": {{}} }}"),
        AssetKind::Executable => format!("\"{base_ref}\""),
    }
}

fn insert_for_asset(asset: &AssetEntry, base_ref: &str) -> Result<PendingInsert, String> {
    let target = match asset.kind {
        AssetKind::Data | AssetKind::Generator => InsertTarget::Binding,
        AssetKind::Assertion => InsertTarget::Assertion,
        AssetKind::Hook | AssetKind::Extractor => InsertTarget::Pipeline,
        AssetKind::Mock => InsertTarget::Mock,
        AssetKind::Executable => {
            return Err("generic executable assets need metadata with a pipeline phase".to_string())
        }
    };
    Ok(PendingInsert {
        target,
        suggested_name: suggested_name(base_ref),
        snippet: snippet_for(asset, base_ref),
    })
}

fn suggested_name(reference: &str) -> String {
    reference
        .split('#')
        .next_back()
        .unwrap_or(reference)
        .trim_matches('/')
        .rsplit(['/', ':'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("asset")
        .trim_end_matches(".json")
        .to_string()
}

fn apply_structured_insert(text: &str, insert: PendingInsert) -> Result<(String, String), String> {
    let mut document = forge_core::reqv1::RequestDocument::parse(text)
        .map_err(|error| format!("fix the request JSON before inserting: {error}"))?;
    let notice = match insert.target {
        InsertTarget::Binding => {
            let binding: Binding = serde_json::from_str(&insert.snippet)
                .map_err(|error| format!("invalid binding: {error}"))?;
            let name = document.insert_binding(&insert.suggested_name, binding);
            format!("Inserted binding “{name}”; reference it as ${{bindings.{name}}}.")
        }
        InsertTarget::Assertion => return Err("assertions belong in the assertion sidecar".into()),
        InsertTarget::Pipeline => return Err("hooks belong in the hook sidecar".to_string()),
        InsertTarget::Mock => {
            let mock: MockDef = serde_json::from_str(&insert.snippet)
                .map_err(|error| format!("invalid mock: {error}"))?;
            document.mock = Some(mock);
            "Set the request mock.".to_string()
        }
    };
    let mut text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok((text, notice))
}

fn apply_insert(
    editor: &mut V1EditorState,
    insert: PendingInsert,
) -> Result<(Option<String>, String), String> {
    if matches!(insert.target, InsertTarget::Assertion) {
        let entry: PipelineEntry = serde_json::from_str(&insert.snippet)
            .map_err(|error| format!("invalid assertion: {error}"))?;
        let mut assertion: AssertionEntry = entry.into();
        if let Some(index) = editor.editing_assertion.take() {
            let current = editor
                .assertions
                .assertions
                .get_mut(index)
                .ok_or_else(|| "the assertion no longer exists".to_string())?;
            assertion.enabled = current.enabled;
            *current = assertion;
            return Ok((None, "Updated the assertion in its sidecar.".to_string()));
        }
        editor.assertions.push(assertion);
        return Ok((
            None,
            "Added the assertion outside the request document.".to_string(),
        ));
    }
    if matches!(insert.target, InsertTarget::Pipeline) {
        let mut hook: PipelineEntry = serde_json::from_str(&insert.snippet)
            .map_err(|error| format!("invalid hook: {error}"))?;
        if let Some(index) = editor.editing_hook.take() {
            let current = editor
                .hooks
                .hooks
                .get_mut(index)
                .ok_or_else(|| "the hook no longer exists".to_string())?;
            hook.enabled = current.enabled;
            *current = hook;
            return Ok((None, "Updated the hook in its sidecar.".to_string()));
        }
        editor.hooks.push(hook);
        return Ok((
            None,
            "Added the hook outside the request document.".to_string(),
        ));
    }
    apply_structured_insert(&editor.text, insert).map(|(text, notice)| (Some(text), notice))
}

fn serialize_request(document: &forge_core::reqv1::RequestDocument) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(document).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}

fn pretty_json(text: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON: {error}"))?;
    let mut text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}

const EDITOR_VALIDATION_DELAY: Duration = Duration::from_millis(180);

fn schedule_editor_validation(d: &mut V1EditorState, ctx: &egui::Context) {
    d.validation_due = Some(Instant::now() + EDITOR_VALIDATION_DELAY);
    ctx.request_repaint_after(EDITOR_VALIDATION_DELAY);
}

fn refresh_editor_validation(d: &mut V1EditorState, ctx: &egui::Context) {
    if d.validated_text == d.text {
        return;
    }
    let now = Instant::now();
    let due = d
        .validation_due
        .get_or_insert(now + EDITOR_VALIDATION_DELAY);
    if now < *due {
        ctx.request_repaint_after(*due - now);
        return;
    }
    validate_editor_json(d);
}

fn validate_editor_json(d: &mut V1EditorState) {
    d.validated_text.clone_from(&d.text);
    d.validation_due = None;
    match forge_core::reqv1::RequestDocument::parse(&d.text) {
        Ok(document) => {
            d.validated_document = Some(document);
            d.json_diagnostic = None;
        }
        Err(error) => {
            d.validated_document = None;
            d.json_diagnostic = Some(EditorDiagnostic {
                line: error.line().max(1),
                column: error.column().max(1),
                message: error.to_string(),
            });
        }
    }
}

fn format_request(d: &mut V1EditorState) {
    match pretty_json(&d.text) {
        Ok(text) => {
            d.text = text;
            d.dirty = true;
            d.diagnostics.clear();
            d.clear_preview();
            validate_editor_json(d);
        }
        Err(error) => {
            d.diagnostics = vec![error];
            d.result_tab = ResultTab::Diagnostics;
        }
    }
}

fn request_editor_footer_height(d: &V1EditorState) -> f32 {
    let diagnostic_height = if d.json_diagnostic.is_some() {
        28.0
    } else {
        0.0
    };
    let assist_height = if d.openapi_error.is_some() || d.openapi.is_none() {
        28.0
    } else if let (Some(spec), Some(document)) = (&d.openapi, &d.validated_document) {
        let matched = spec
            .find_operation(document.request.method, &document.request.url)
            .is_some();
        if matched || spec.suggest(&document.request.url).is_empty() {
            28.0
        } else {
            64.0
        }
    } else {
        0.0
    };
    diagnostic_height + assist_height
}

fn openapi_assist(ui: &mut egui::Ui, d: &mut V1EditorState, response: &egui::Response) {
    if let Some(error) = &d.openapi_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
        return;
    }
    let Some(spec) = &d.openapi else {
        ui.label(
            RichText::new("OpenAPI completion: add openapi.yaml or a spec under specs/.")
                .small()
                .weak(),
        );
        return;
    };
    let Some(document) = d.validated_document.clone() else {
        return;
    };
    let source = d
        .openapi_source
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let matched = spec
        .find_operation(document.request.method, &document.request.url)
        .cloned();
    let suggestions = if matched.is_none() {
        spec.suggest(&document.request.url)
            .into_iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut selected = None;

    if let Some(operation) = matched {
        let issues = openapi_request_issues(&document, &operation);
        ui.horizontal_wrapped(|ui| {
            if issues.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(0x47, 0xC9, 0x82),
                    format!(
                        "OpenAPI · {} {} · {source}",
                        operation.method.as_str(),
                        operation.path
                    ),
                );
            } else {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{} OpenAPI: {}", icons::WARNING, issues.join(" · ")),
                );
                if ui.small_button("Apply fixes").clicked() {
                    selected = Some(operation.clone());
                }
            }
        });
    } else {
        let allowed = spec
            .operations
            .iter()
            .filter(|operation| {
                forge_core::openapi::path_matches_template(
                    &operation.path,
                    &forge_core::openapi::url_to_path(&document.request.url),
                )
            })
            .map(|operation| operation.method.as_str())
            .collect::<Vec<_>>();
        let message = if allowed.is_empty() {
            "Path is not declared in OpenAPI".to_string()
        } else {
            format!("Method not allowed; use {}", allowed.join(", "))
        };
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("{} {message}", icons::WARNING),
        );
        if !suggestions.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Suggestions (Tab selects first):");
                for operation in &suggestions {
                    if ui
                        .small_button(format!("{} {}", operation.method.as_str(), operation.path))
                        .on_hover_text(&operation.summary)
                        .clicked()
                    {
                        selected = Some(operation.clone());
                    }
                }
            });
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Tab))
            {
                selected = suggestions.first().cloned();
            }
        }
    }

    if let Some(operation) = selected {
        match apply_openapi_operation(&d.text, &operation) {
            Ok(text) => {
                d.text = text;
                d.dirty = true;
                d.diagnostics.clear();
                d.clear_preview();
            }
            Err(error) => {
                d.diagnostics = vec![error];
                d.result_tab = ResultTab::Diagnostics;
            }
        }
    }
}

fn openapi_request_issues(
    document: &forge_core::reqv1::RequestDocument,
    operation: &SpecOperation,
) -> Vec<String> {
    let mut issues = Vec::new();
    for name in &operation.path_params {
        if document
            .request
            .url
            .contains(&format!("${{bindings.{name}}}"))
            && !document.bindings.contains_key(name)
        {
            issues.push(format!("missing path binding {name}"));
        }
    }
    for (name, required) in &operation.query_params {
        if *required
            && !document
                .request
                .query
                .iter()
                .any(|parameter| parameter.enabled && parameter.name.eq_ignore_ascii_case(name))
        {
            issues.push(format!("missing query {name}"));
        }
    }
    for (name, required) in &operation.header_params {
        if *required
            && !document
                .request
                .headers
                .iter()
                .any(|header| header.enabled && header.name.eq_ignore_ascii_case(name))
        {
            issues.push(format!("missing header {name}"));
        }
    }
    if let Some(content_type) = &operation.request_content_type {
        let matches = document.request.headers.iter().any(|header| {
            header.enabled
                && header.name.eq_ignore_ascii_case("content-type")
                && header
                    .value
                    .split(';')
                    .next()
                    .is_some_and(|value| value.eq_ignore_ascii_case(content_type))
        });
        if !matches {
            issues.push(format!("content type should be {content_type}"));
        }
    }
    if let (Some(schema), Some(BodySpec::Inline(body))) =
        (&operation.request_schema, &document.request.body)
    {
        if let Some(value) = &body.value {
            let schema = forge_core::openapi::scrub_schema(schema.clone());
            if let Err(errors) = forge_core::assert::schema::validate(&schema, value) {
                issues.push(format!("body: {}", errors.join(", ")));
            }
        }
    }
    issues
}

fn apply_openapi_operation(text: &str, operation: &SpecOperation) -> Result<String, String> {
    let mut document = forge_core::reqv1::RequestDocument::parse(text)
        .map_err(|error| format!("fix the request JSON before completion: {error}"))?;
    document.request.method = operation.method;
    let prefix = request_url_prefix(&document.request.url);
    let mut path = operation.path.clone();
    for name in &operation.path_params {
        path = path.replace(&format!("{{{name}}}"), &format!("${{bindings.{name}}}"));
        document
            .bindings
            .entry(name.clone())
            .or_insert_with(|| Binding::Value(ValueBinding { value: "".into() }));
    }
    document.request.url = format!("{prefix}{path}");
    for (name, required) in &operation.query_params {
        if *required
            && !document
                .request
                .query
                .iter()
                .any(|parameter| parameter.name.eq_ignore_ascii_case(name))
        {
            document.request.query.push(HeaderSpec {
                name: name.clone(),
                value: String::new(),
                enabled: true,
            });
        }
    }
    for (name, required) in &operation.header_params {
        if *required
            && !document
                .request
                .headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case(name))
        {
            document.request.headers.push(HeaderSpec {
                name: name.clone(),
                value: String::new(),
                enabled: true,
            });
        }
    }
    if let Some(content_type) = &operation.request_content_type {
        if !document
            .request
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("content-type"))
        {
            document.request.headers.push(HeaderSpec {
                name: "Content-Type".to_string(),
                value: content_type.clone(),
                enabled: true,
            });
        }
    }
    if document.request.body.is_none() {
        let value = operation.request_example.clone().or_else(|| {
            operation
                .request_schema
                .as_ref()
                .map(|schema| forge_core::openapi::example_from_schema(schema, 0))
        });
        if let Some(value) = value {
            document.request.body = Some(BodySpec::Inline(InlineBody {
                body_type: BodyType::Json,
                value: Some(value),
            }));
        }
    }
    let mut text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}

fn apply_generated_openapi_values(text: &str, operation: &SpecOperation) -> Result<String, String> {
    let mut document = forge_core::reqv1::RequestDocument::parse(text)
        .map_err(|error| format!("fix the request JSON before completion: {error}"))?;
    for name in &operation.path_params {
        document.bindings.insert(
            name.clone(),
            Binding::Value(ValueBinding {
                value: "sample".into(),
            }),
        );
    }
    for (name, required) in &operation.query_params {
        if *required {
            if let Some(parameter) = document
                .request
                .query
                .iter_mut()
                .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            {
                parameter.value = "sample".to_string();
            }
        }
    }
    for (name, required) in &operation.header_params {
        if *required {
            if let Some(header) = document
                .request
                .headers
                .iter_mut()
                .find(|header| header.name.eq_ignore_ascii_case(name))
            {
                header.value = "sample".to_string();
            }
        }
    }
    if let Some(value) = operation.request_example.clone().or_else(|| {
        operation
            .request_schema
            .as_ref()
            .map(|schema| forge_core::openapi::example_from_schema(schema, 0))
    }) {
        document.request.body = Some(BodySpec::Inline(InlineBody {
            body_type: BodyType::Json,
            value: Some(value),
        }));
    }
    serialize_request(&document)
}

fn marked_operations_path(root: &Path) -> PathBuf {
    root.join(".forge-local/openapi-covered.json")
}

fn load_marked_operations(root: &Path) -> Result<BTreeSet<String>, String> {
    let path = marked_operations_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn save_marked_operations(root: &Path, operations: &BTreeSet<String>) -> Result<(), String> {
    let path = marked_operations_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(operations).map_err(|error| error.to_string())?;
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn request_url_prefix(url: &str) -> &str {
    if let Some(rest) = url.strip_prefix("${") {
        return rest
            .find('}')
            .map(|end| &url[..end + 3])
            .unwrap_or_default();
    }
    if let Some(rest) = url.strip_prefix("{{") {
        return rest
            .find("}}")
            .map(|end| &url[..end + 4])
            .unwrap_or_default();
    }
    if let Some(scheme) = url.find("://") {
        let host = scheme + 3;
        return url[host..]
            .find('/')
            .map(|slash| &url[..host + slash])
            .unwrap_or(url);
    }
    ""
}

/// Bottom of the split: run output and request-adjacent behavior stay in
/// separate tabs instead of bloating the request document.
fn scale_results_typography(style: &mut egui::Style, editor_font_size: f32) {
    let scale = editor_font_size / crate::state::DEFAULT_EDITOR_FONT_SIZE;
    for text_style in [
        egui::TextStyle::Body,
        egui::TextStyle::Button,
        egui::TextStyle::Small,
        egui::TextStyle::Monospace,
        egui::TextStyle::Heading,
    ] {
        if let Some(font) = style.text_styles.get_mut(&text_style) {
            font.size *= scale;
        }
    }
}

fn results_pane(ui: &mut egui::Ui, d: &mut V1EditorState, editor_font_size: f32) {
    scale_results_typography(ui.style_mut(), editor_font_size);
    if d.results.len() > 1 {
        let mut selected = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Runs:");
            for (index, item) in d.results.iter().enumerate() {
                if ui
                    .selectable_label(d.selected_result == index, &item.label)
                    .clicked()
                {
                    selected = Some(index);
                }
            }
        });
        if let Some(index) = selected {
            d.selected_result = index;
            d.last_response = d.results[index].response.clone();
            d.clear_preview();
        }
        ui.separator();
    }
    // Tab strip with a pass/fail count on the Assertions tab.
    let (passed, total) = selected_result(d)
        .map(|r| {
            (
                r.assertions.iter().filter(|a| a.passed).count(),
                r.assertions.len(),
            )
        })
        .unwrap_or((0, 0));
    let assertions_label = if total > 0 {
        format!("Assertions ({passed}/{total})")
    } else {
        "Assertions".to_string()
    };
    let hooks_label = if d.hooks.hooks.is_empty() {
        "Hooks".to_string()
    } else {
        format!("Hooks ({})", d.hooks.hooks.len())
    };
    let auth_label = if d.project_auth.is_some() {
        "Auth · active"
    } else {
        "Auth"
    };

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = if ui.available_width() < 640.0 {
            12.0
        } else {
            18.0
        };
        let compact = ui.available_width() < 640.0;
        let tabs = [
            (ResultTab::Result, "Response".to_string()),
            (ResultTab::Assertions, assertions_label),
            (ResultTab::Hooks, hooks_label),
            (ResultTab::Auth, auth_label.to_string()),
            (ResultTab::Runtime, "Runtime".to_string()),
            (ResultTab::Trace, "Trace".to_string()),
            (ResultTab::Diagnostics, "Diagnostics".to_string()),
        ];
        for (which, label) in tabs.iter().take(if compact { 4 } else { tabs.len() }) {
            let active = d.result_tab == *which;
            let text = RichText::new(label).color(if active {
                ui.visuals().hyperlink_color
            } else {
                ui.visuals().weak_text_color()
            });
            let response = ui
                .add(egui::Label::new(text).sense(egui::Sense::click()))
                .on_hover_text(result_tab_help(*which));
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
                d.result_tab = *which;
            }
        }
        if compact {
            let overflow = &tabs[4..];
            let selected = overflow
                .iter()
                .find(|(tab, _)| *tab == d.result_tab)
                .map(|(_, label)| format!("{}  {label}", icons::ELLIPSIS))
                .unwrap_or_else(|| format!("{}  More", icons::ELLIPSIS));
            ui.menu_button(selected, |ui| {
                for (tab, label) in overflow {
                    if ui
                        .selectable_label(d.result_tab == *tab, label)
                        .on_hover_text(result_tab_help(*tab))
                        .clicked()
                    {
                        d.result_tab = *tab;
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Show additional response detail tabs");
        }
    });
    ui.add_space(4.0);
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("v1-results")
        .auto_shrink([false, false])
        .show(ui, |ui| match d.result_tab {
            ResultTab::Result => result_summary(ui, d),
            ResultTab::Assertions => assertions_pane(ui, d),
            ResultTab::Hooks => hooks_pane(ui, d),
            ResultTab::Auth => auth_pane(ui, d),
            ResultTab::Runtime => runtime_pane(ui, d),
            ResultTab::Trace => trace_pane(ui),
            ResultTab::Diagnostics => diagnostics_pane(ui, d),
        });
}

fn result_tab_help(tab: ResultTab) -> &'static str {
    match tab {
        ResultTab::Result => "Formatted response body, headers and status",
        ResultTab::Assertions => "Assertions configured for this request and their results",
        ResultTab::Hooks => "Before- and after-request scripts",
        ResultTab::Auth => "Authentication source, refresh policy and token status",
        ResultTab::Runtime => "Execution duration, environment and transport details",
        ResultTab::Trace => "Request lifecycle trace (coming soon)",
        ResultTab::Diagnostics => "Validation, OpenAPI and execution diagnostics",
    }
}

fn result_summary(ui: &mut egui::Ui, d: &mut V1EditorState) {
    let Some(item) = selected_item(d) else {
        ui.allocate_ui_with_layout(
            ui.available_size(),
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icons::RUN).size(22.0).weak());
                    ui.strong("No response yet");
                });
            },
        );
        return;
    };
    let r = &item.result;
    if !item.matrix.is_empty() {
        ui.monospace(serde_json::Value::Object(item.matrix.clone()).to_string());
    }
    let (label, color) = match r.status {
        RunStatus::Passed => ("PASSED", egui::Color32::from_rgb(0x49, 0x9C, 0x54)),
        RunStatus::Failed => ("FAILED", egui::Color32::from_rgb(0xC7, 0x5A, 0x3B)),
        RunStatus::Error => ("ERROR", ui.visuals().error_fg_color),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(color).strong());
        if let Some(http) = &r.http {
            ui.label(format!(
                "{} · {} ms · {} bytes",
                http.status, http.time_ms, http.bytes
            ));
        }
    });
    let (passed, total) = (
        r.assertions.iter().filter(|a| a.passed).count(),
        r.assertions.len(),
    );
    if total > 0 {
        ui.label(format!("{passed}/{total} assertion(s) passed"));
    }
    if !r.runtime.is_empty() {
        ui.label(format!("{} runtime value(s) extracted", r.runtime.len()));
    }
    let Some(response) = d.last_response.clone() else {
        return;
    };
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("HTTP {}", response.status));
        ui.weak(format!(
            "{} ms · {} bytes",
            response.time_ms,
            response.body.len()
        ));
        ui.checkbox(&mut d.response_raw, "Raw");
        if ui.button(format!("{}  Copy", icons::COPY)).clicked() {
            ui.ctx().copy_text(response.text().into_owned());
        }
    });
    for issue in openapi_response_issues(d, &response) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("{} {issue}", icons::WARNING),
        );
    }
    let response_text = response.text().into_owned();
    let json = response.json();
    let markup = response_is_markup(&response, &response_text);
    let mut body = if d.response_raw {
        response_text
    } else if let Some(value) = json.as_ref() {
        serde_json::to_string_pretty(value).unwrap_or(response_text)
    } else if markup {
        pretty_markup(&response_text)
    } else {
        response_text
    };
    let language = if json.is_some() {
        Lang::Json
    } else if markup {
        Lang::Xml
    } else {
        Lang::Plain
    };
    egui::ScrollArea::both()
        .id_salt("v1-response-body")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            code_editor_numbered(
                ui,
                "v1-response-json",
                &mut body,
                language,
                None,
                true,
                8,
                false,
            );
        });
}

fn trace_pane(ui: &mut egui::Ui) {
    ui.allocate_ui_with_layout(
        ui.available_size(),
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
        |ui| {
            ui.strong("No trace captured").on_hover_text(
                "Trace will show request phases, hooks, redirects, and network timing.",
            );
        },
    );
}

fn response_is_markup(response: &ResponseView, text: &str) -> bool {
    let starts_with_markup = text.trim_start().to_ascii_lowercase();
    response.header("content-type").is_some_and(|content_type| {
        let content_type = content_type.to_ascii_lowercase();
        content_type.contains("html") || content_type.contains("xml")
    }) || starts_with_markup.starts_with("<!doctype")
        || starts_with_markup.starts_with("<html")
        || starts_with_markup.starts_with("<?xml")
}

fn pretty_markup(input: &str) -> String {
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];
    let mut output = String::new();
    let mut depth = 0usize;
    let mut rest = input.trim();
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let Some(end) = markup_tag_end(rest) else {
                push_markup_line(&mut output, depth, rest);
                break;
            };
            let tag = &rest[..=end];
            let closing = tag.starts_with("</");
            if closing {
                depth = depth.saturating_sub(1);
            }
            push_markup_line(&mut output, depth, tag);
            let name = tag
                .trim_start_matches(['<', '/'])
                .split(|character: char| {
                    character.is_whitespace() || character == '>' || character == '/'
                })
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if !closing
                && !tag.starts_with("<!")
                && !tag.starts_with("<?")
                && !tag.ends_with("/>")
                && !VOID.contains(&name.as_str())
            {
                depth += 1;
            }
            rest = rest[end + 1..].trim_start();
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            push_markup_line(&mut output, depth, rest[..end].trim());
            rest = rest[end..].trim_start();
        }
    }
    output
}

fn markup_tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in tag.char_indices().skip(1) {
        match character {
            '\'' | '"' if quote.is_none() => quote = Some(character),
            character if quote == Some(character) => quote = None,
            '>' if quote.is_none() => return Some(index),
            _ => {}
        }
    }
    None
}

fn push_markup_line(output: &mut String, depth: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(&"  ".repeat(depth));
    output.push_str(text);
}

fn openapi_response_issues(d: &V1EditorState, response: &ResponseView) -> Vec<String> {
    let (Some(spec), Ok(document)) = (
        d.openapi.as_ref(),
        forge_core::reqv1::RequestDocument::parse(&d.text),
    ) else {
        return Vec::new();
    };
    let Some(operation) = spec.find_operation(document.request.method, &document.request.url)
    else {
        return vec!["Response cannot be matched to an OpenAPI operation".to_string()];
    };
    let Some(declared) = declared_response(&operation.responses, response.status) else {
        return vec![format!(
            "Status {} is not declared by OpenAPI",
            response.status
        )];
    };
    let mut issues = Vec::new();
    if let Some(expected) = &declared.content_type {
        let actual = response
            .header("content-type")
            .and_then(|value| value.split(';').next());
        if actual.is_none_or(|actual| !actual.eq_ignore_ascii_case(expected)) {
            issues.push(format!(
                "Response content type is {}, expected {expected}",
                actual.unwrap_or("missing")
            ));
        }
    }
    if let Some(schema) = &declared.schema {
        match response.json() {
            Some(body) => {
                let schema = forge_core::openapi::scrub_schema(schema.clone());
                if let Err(errors) = forge_core::assert::schema::validate(&schema, &body) {
                    issues.push(format!("Response body: {}", errors.join(", ")));
                }
            }
            None => issues.push("Response body is not valid JSON".to_string()),
        }
    }
    issues
}

fn declared_response(responses: &[SpecResponse], status: u16) -> Option<&SpecResponse> {
    let exact = status.to_string();
    responses
        .iter()
        .find(|response| response.status == exact)
        .or_else(|| {
            let class = format!("{}XX", status / 100);
            responses
                .iter()
                .find(|response| response.status.eq_ignore_ascii_case(&class))
        })
        .or_else(|| {
            responses
                .iter()
                .find(|response| response.status == "default")
        })
}

fn assertions_pane(ui: &mut egui::Ui, d: &mut V1EditorState) {
    let mut add = None;
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.strong(format!("Configured ({})", d.assertions.assertions.len()));
        ui.menu_button("+ Add assertion", |ui| {
            for definition in builtin_catalog()
                .iter()
                .filter(|definition| definition.intent == BuiltinIntent::Validate)
            {
                if ui.button(definition.title).clicked() {
                    add = Some(*definition);
                    ui.close();
                }
            }
        });
    });
    if let Some(file) = d.file.as_deref() {
        ui.weak(format!(
            "Stored in {}",
            forge_core::reqv1::assertions_path(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    if let Some(definition) = add {
        match assertion_from_builtin(&definition) {
            Ok(assertion) => {
                d.assertions.push(assertion.clone());
                d.dirty = true;
                edit = d
                    .assertions
                    .assertions
                    .iter()
                    .position(|candidate| candidate == &assertion);
            }
            Err(error) => d.diagnostics.push(error),
        }
    }

    let mut remove = None;
    let mut changed = false;
    for (index, assertion) in d.assertions.assertions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut assertion.enabled, "").changed();
            ui.label(RichText::new(catalog_entry_title(&assertion.uses)).strong());
            ui.weak(&assertion.uses);
            if ui.small_button("Edit in catalog").clicked() {
                edit = Some(index);
            }
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
        if !assertion.with.is_empty() {
            ui.label(
                RichText::new(serde_json::Value::Object(assertion.with.clone()).to_string())
                    .monospace()
                    .small()
                    .weak(),
            );
        }
    }
    if let Some(index) = remove {
        d.assertions.assertions.remove(index);
        d.editing_assertion = match d.editing_assertion {
            Some(editing) if editing == index => None,
            Some(editing) if editing > index => Some(editing - 1),
            editing => editing,
        };
        changed = true;
    }
    if let Some(index) = edit {
        if let Err(error) = begin_assertion_edit(d, index) {
            d.catalog_error = Some(error);
        }
    }
    if changed {
        d.dirty = true;
    }
    if d.assertions.assertions.is_empty() {
        ui.weak("No assertions configured. Add a built-in here or configure one in the catalog.");
    }

    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("LAST RUN").small().strong().weak());
    let Some(r) = selected_result(d) else {
        ui.weak("Run the request to see assertion results.");
        return;
    };
    if r.assertions.is_empty() {
        ui.weak("No assertion results.");
        return;
    }
    for a in &r.assertions {
        let (mark, color) = if a.passed {
            ("✓", egui::Color32::from_rgb(0x49, 0x9C, 0x54))
        } else {
            ("✗", ui.visuals().error_fg_color)
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(mark).color(color).strong());
            ui.label(&a.message);
        });
        if !a.passed {
            if let Some(exp) = &a.expected {
                ui.label(RichText::new(format!("    expected: {exp}")).small().weak());
            }
            if let Some(act) = &a.actual {
                ui.label(RichText::new(format!("    actual:   {act}")).small().weak());
            }
        }
    }
}

fn hooks_pane(ui: &mut egui::Ui, d: &mut V1EditorState) {
    let mut add = None;
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.strong(format!("Configured ({})", d.hooks.hooks.len()));
        ui.menu_button("+ Add hook", |ui| {
            for definition in builtin_catalog().iter().filter(|definition| {
                matches!(
                    definition.intent,
                    BuiltinIntent::Prepare | BuiltinIntent::Capture
                ) && matches!(definition.target, BuiltinTarget::Pipeline(_))
            }) {
                if ui.button(definition.title).clicked() {
                    add = Some(*definition);
                    ui.close();
                }
            }
        });
    });
    if let Some(file) = d.file.as_deref() {
        ui.weak(format!(
            "Stored in {}",
            forge_core::reqv1::hooks_path(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    if let Some(definition) = add {
        match hook_from_builtin(&definition) {
            Ok(hook) => {
                d.hooks.push(hook.clone());
                d.dirty = true;
                edit = d.hooks.hooks.iter().position(|candidate| {
                    candidate.phase == hook.phase
                        && candidate.uses == hook.uses
                        && candidate.with == hook.with
                        && candidate.enabled == hook.enabled
                });
            }
            Err(error) => d.diagnostics.push(error),
        }
    }

    let mut remove = None;
    let mut changed = false;
    for (index, hook) in d.hooks.hooks.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut hook.enabled, "").changed();
            ui.label(RichText::new(catalog_entry_title(&hook.uses)).strong());
            ui.weak(target_label(BuiltinTarget::Pipeline(hook.phase)));
            ui.weak(&hook.uses);
            if ui.small_button("Edit in catalog").clicked() {
                edit = Some(index);
            }
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
        if !hook.with.is_empty() {
            ui.label(
                RichText::new(serde_json::Value::Object(hook.with.clone()).to_string())
                    .monospace()
                    .small()
                    .weak(),
            );
        }
    }
    if let Some(index) = remove {
        d.hooks.hooks.remove(index);
        d.editing_hook = match d.editing_hook {
            Some(editing) if editing == index => None,
            Some(editing) if editing > index => Some(editing - 1),
            editing => editing,
        };
        changed = true;
    }
    if let Some(index) = edit {
        if let Err(error) = begin_hook_edit(d, index) {
            d.catalog_error = Some(error);
        }
    }
    if changed {
        d.dirty = true;
    }
    if d.hooks.hooks.is_empty() {
        ui.weak("No hooks configured. Add a built-in here or configure one in the catalog.");
    }
}

fn auth_pane(ui: &mut egui::Ui, d: &mut V1EditorState) {
    ui.heading("Authentication").on_hover_text(
        "Fetches and caches a token, then refreshes it before a protected request can outlive it.",
    );
    ui.add_space(8.0);

    let current = current_request_path(d);
    let requests: Vec<(String, String)> = d
        .index
        .as_ref()
        .map(|index| {
            index
                .requests
                .iter()
                .map(|request| (request.rel_path.clone(), request.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    if !requests
        .iter()
        .any(|(path, _)| path == &d.auth_request_choice)
    {
        d.auth_request_choice = d
            .project_auth
            .as_ref()
            .map(|auth| auth.request.clone())
            .filter(|path| requests.iter().any(|(candidate, _)| candidate == path))
            .or_else(|| requests.first().map(|(path, _)| path.clone()))
            .unwrap_or_default();
    }

    let mut activate = None;
    let mut create = false;
    let mut save = false;
    let mut disable = false;

    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(7)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(auth) = &d.project_auth {
                    ui.label(RichText::new(format!("{}  Active", icons::CHECK)).strong());
                    ui.monospace(&auth.request);
                    if ui.small_button("Disable").clicked() {
                        disable = true;
                    }
                } else {
                    ui.label("Not configured");
                }
                if ui
                    .add_enabled(
                        current.is_some() && !d.new_file,
                        egui::Button::new("Use current request"),
                    )
                    .on_disabled_hover_text("Save this request first")
                    .clicked()
                {
                    activate = current.clone();
                }
            });
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.strong("Source");
        egui::ComboBox::from_id_salt("auth-setup-source")
            .selected_text(d.auth_setup.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut d.auth_setup,
                    AuthSetup::ExistingRequest,
                    AuthSetup::ExistingRequest.label(),
                );
                ui.selectable_value(
                    &mut d.auth_setup,
                    AuthSetup::Provider,
                    AuthSetup::Provider.label(),
                );
            });
    });

    match d.auth_setup {
        AuthSetup::ExistingRequest => {
            ui.horizontal_wrapped(|ui| {
                let selected = requests
                    .iter()
                    .find(|(path, _)| path == &d.auth_request_choice)
                    .map(|(path, name)| format!("{name} — {path}"))
                    .unwrap_or_else(|| "Select a request".to_string());
                egui::ComboBox::from_id_salt("auth-request-choice")
                    .selected_text(selected)
                    .width(320.0)
                    .show_ui(ui, |ui| {
                        for (path, name) in &requests {
                            ui.selectable_value(
                                &mut d.auth_request_choice,
                                path.clone(),
                                format!("{name} — {path}"),
                            );
                        }
                    });
                if ui
                    .add_enabled(
                        !d.auth_request_choice.is_empty(),
                        egui::Button::new("Use selected"),
                    )
                    .clicked()
                {
                    activate = Some(d.auth_request_choice.clone());
                }
            });
        }
        AuthSetup::Provider => {
            egui::Grid::new("auth-provider-form")
                .num_columns(2)
                .spacing([12.0, 7.0])
                .show(ui, |ui| {
                    ui.label("Provider");
                    egui::ComboBox::from_id_salt("auth-provider")
                        .selected_text(d.auth_draft.provider.label())
                        .show_ui(ui, |ui| {
                            for provider in AuthProvider::ALL {
                                ui.selectable_value(
                                    &mut d.auth_draft.provider,
                                    provider,
                                    provider.label(),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label(d.auth_draft.provider.endpoint_label());
                    ui.text_edit_singleline(&mut d.auth_draft.endpoint);
                    ui.end_row();

                    if d.auth_draft.provider == AuthProvider::Keycloak {
                        ui.label("Realm");
                        ui.text_edit_singleline(&mut d.auth_draft.realm);
                        ui.end_row();
                    }

                    ui.label("Client ID");
                    ui.text_edit_singleline(&mut d.auth_draft.client_id);
                    ui.end_row();

                    ui.label("Client secret");
                    ui.add(TextEdit::singleline(&mut d.auth_draft.client_secret).password(true));
                    ui.end_row();

                    ui.label(d.auth_draft.provider.scope_label());
                    ui.text_edit_singleline(&mut d.auth_draft.scope);
                    ui.end_row();
                });
            if ui
                .button("Create and use auth request")
                .on_hover_text(format!(
                    "Stores the secret locally as {} in .env.local. Leave it empty to reuse an environment value.",
                    d.auth_draft.provider.secret_name()
                ))
                .clicked()
            {
                create = true;
            }
        }
    }

    if let Some(auth) = d.project_auth.as_mut() {
        ui.separator();
        egui::CollapsingHeader::new("Token and refresh settings")
            .id_salt("auth-runtime-settings")
            .show(ui, |ui| {
                let mut changed = false;
                egui::Grid::new("project-auth-form")
                    .num_columns(2)
                    .spacing([12.0, 7.0])
                    .show(ui, |ui| {
                        ui.label("Token JSONPath");
                        changed |= ui.text_edit_singleline(&mut auth.token_path).changed();
                        ui.end_row();

                        ui.label("Lifetime");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut auth.lifetime_seconds)
                                    .range(1..=31_536_000)
                                    .suffix(" s"),
                            )
                            .changed();
                        ui.end_row();

                        ui.label("Refresh reserve");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut auth.refresh_before_seconds)
                                    .range(0..=31_536_000)
                                    .suffix(" s"),
                            )
                            .changed();
                        ui.end_row();

                        ui.label("Apply to");
                        changed |= ui.text_edit_singleline(&mut auth.apply_to).changed();
                        ui.end_row();
                    });
                if changed {
                    d.auth_dirty = true;
                    d.auth_notice = None;
                }
                ui.label("Scope rules").on_hover_text(
                    "Apply to accepts a project-relative request folder or file. Explicit Authorization headers win.",
                );
                if ui
                    .add_enabled(d.auth_dirty, egui::Button::new("Save settings"))
                    .clicked()
                {
                    save = true;
                }
            });
    }

    if disable {
        d.project_auth = None;
        d.auth_dirty = true;
        save = true;
    }
    if let Some(request) = activate {
        activate_auth_request(d, request);
    }
    if create {
        create_provider_auth_request(d);
    }
    if save {
        save_project_auth(d);
    }
    if let Some(notice) = &d.auth_notice {
        ui.label(notice);
    }
}

fn current_request_path(d: &V1EditorState) -> Option<String> {
    let root = d.root.as_ref()?;
    let file = d.file.as_ref()?;
    Some(
        file.strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

fn activate_auth_request(d: &mut V1EditorState, request: String) {
    let mut auth = d
        .project_auth
        .clone()
        .unwrap_or_else(|| ProjectAuthConfig::for_request(request.clone()));
    auth.request = request;
    d.project_auth = Some(auth);
    d.auth_dirty = true;
    d.auth_notice = None;
    save_project_auth(d);
}

fn create_provider_auth_request(d: &mut V1EditorState) {
    let result = (|| {
        let root = d
            .root
            .as_deref()
            .ok_or_else(|| "no project root".to_string())?;
        let directory = root.join("requests/auth");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let path = forge_core::reqv1::available_path(
            &directory,
            d.auth_draft.provider.file_stem(),
            ".request.json",
        );
        let stem = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".request.json"))
            .ok_or_else(|| "cannot derive auth request name".to_string())?;
        let document = provider_auth_document(&d.auth_draft, stem)?;
        let text = serialize_request(&document)?;
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;

        let after_write: Result<(String, ProjectAuthConfig), String> = (|| {
            if !d.auth_draft.client_secret.is_empty() {
                save_local_secret(
                    root,
                    d.auth_draft.provider.secret_name(),
                    &d.auth_draft.client_secret,
                )?;
            }
            let request = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let mut auth = d
                .project_auth
                .clone()
                .unwrap_or_else(|| ProjectAuthConfig::for_request(request.clone()));
            auth.request = request.clone();
            persist_project_auth(root, Some(&auth))?;
            Ok((request, auth))
        })();
        if after_write.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        after_write
    })();

    match result {
        Ok((request, auth)) => {
            d.project_auth = Some(auth);
            d.auth_request_choice = request.clone();
            d.auth_draft.client_secret.clear();
            d.auth_dirty = false;
            d.auth_notice = Some(format!("Created and activated {request}."));
            if let Err(error) = d.load_index() {
                d.diagnostics.push(error);
            }
        }
        Err(error) => d.auth_notice = Some(format!("Auth request not created: {error}")),
    }
}

fn provider_auth_document(
    draft: &AuthDraft,
    stem: &str,
) -> Result<forge_core::reqv1::RequestDocument, String> {
    let token_url = provider_token_url(draft)?;
    if draft.client_id.trim().is_empty() {
        return Err("client ID must not be empty".to_string());
    }
    if matches!(draft.provider, AuthProvider::Auth0 | AuthProvider::Entra)
        && draft.scope.trim().is_empty()
    {
        return Err(format!(
            "{} must not be empty",
            draft.provider.scope_label().to_lowercase()
        ));
    }
    let mut form = serde_json::Map::from_iter([
        (
            "grant_type".to_string(),
            serde_json::Value::String("client_credentials".to_string()),
        ),
        (
            "client_id".to_string(),
            serde_json::Value::String(draft.client_id.trim().to_string()),
        ),
        (
            "client_secret".to_string(),
            serde_json::Value::String(format!("${{secret.{}}}", draft.provider.secret_name())),
        ),
    ]);
    if !draft.scope.trim().is_empty() {
        form.insert(
            if draft.provider == AuthProvider::Auth0 {
                "audience"
            } else {
                "scope"
            }
            .to_string(),
            serde_json::Value::String(draft.scope.trim().to_string()),
        );
    }
    let value = serde_json::json!({
        "formatVersion": 1,
        "kind": "request",
        "meta": {
            "id": format!("auth.{stem}"),
            "name": format!("{} token", draft.provider.label())
        },
        "request": {
            "method": "POST",
            "url": token_url,
            "body": { "type": "form", "value": form }
        }
    });
    forge_core::reqv1::RequestDocument::parse(&value.to_string()).map_err(|error| error.to_string())
}

fn provider_token_url(draft: &AuthDraft) -> Result<String, String> {
    let mut url = match draft.provider {
        AuthProvider::Generic => parse_http_url(&draft.endpoint, "token URL")?,
        AuthProvider::Keycloak => {
            if draft.realm.trim().is_empty() {
                return Err("realm must not be empty".to_string());
            }
            let mut url = parse_http_url(&draft.endpoint, "server URL")?;
            url.set_query(None);
            url.set_fragment(None);
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| "Keycloak server URL cannot be a base URL".to_string())?;
            segments.pop_if_empty();
            segments.extend([
                "realms",
                draft.realm.trim(),
                "protocol",
                "openid-connect",
                "token",
            ]);
            drop(segments);
            url
        }
        AuthProvider::Auth0 => {
            let mut url = parse_http_url(&draft.endpoint, "domain")?;
            url.set_path("/oauth/token");
            url.set_query(None);
            url.set_fragment(None);
            url
        }
        AuthProvider::Entra => {
            if draft.endpoint.trim().is_empty() {
                return Err("tenant ID must not be empty".to_string());
            }
            let mut url = url::Url::parse("https://login.microsoftonline.com/")
                .map_err(|error| error.to_string())?;
            url.path_segments_mut()
                .map_err(|_| "invalid Microsoft authority URL".to_string())?
                .extend([draft.endpoint.trim(), "oauth2", "v2.0", "token"]);
            url
        }
    };
    url.set_fragment(None);
    Ok(url.to_string())
}

fn parse_http_url(value: &str, label: &str) -> Result<url::Url, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    let value = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let url = url::Url::parse(&value).map_err(|error| format!("invalid {label}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{label} must be an HTTP(S) URL"));
    }
    Ok(url)
}

fn save_local_secret(root: &Path, name: &str, value: &str) -> Result<(), String> {
    forge_core::store::ensure_gitignore(root).map_err(|error| error.to_string())?;
    let path = root.join(".env.local");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let encoded = serde_json::to_string(value).map_err(|error| error.to_string())?;
    let mut found = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|line| {
            let matches = line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == name);
            if matches {
                found = true;
                format!("{name}={encoded}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{name}={encoded}"));
    }
    std::fs::write(&path, lines.join("\n") + "\n")
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, permissions)
            .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
    }
    Ok(())
}

fn persist_project_auth(root: &Path, auth: Option<&ProjectAuthConfig>) -> Result<(), String> {
    if let Some(auth) = auth {
        auth.validate()?;
    }
    let mut project =
        forge_core::reqv1::load_project(root).map_err(|diagnostic| diagnostic.message)?;
    project.auth = auth.cloned();
    let mut text = serde_json::to_string_pretty(&project).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(root.join("project.json"), text).map_err(|error| error.to_string())
}

fn save_project_auth(d: &mut V1EditorState) {
    let result = (|| {
        let root = d
            .root
            .as_deref()
            .ok_or_else(|| "no project root".to_string())?;
        persist_project_auth(root, d.project_auth.as_ref())
    })();
    match result {
        Ok(()) => {
            d.auth_dirty = false;
            d.auth_notice = Some("Project auth saved.".to_string());
        }
        Err(error) => d.auth_notice = Some(format!("Auth not saved: {error}")),
    }
}

fn assertion_from_builtin(definition: &BuiltinDefinition) -> Result<AssertionEntry, String> {
    let with = serde_json::from_str::<serde_json::Value>(definition.example)
        .map_err(|error| format!("invalid catalog example for {}: {error}", definition.name))?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("catalog example for {} is not an object", definition.name))?;
    Ok(AssertionEntry {
        uses: definition.reference.to_string(),
        with,
        enabled: true,
    })
}

fn hook_from_builtin(definition: &BuiltinDefinition) -> Result<PipelineEntry, String> {
    let BuiltinTarget::Pipeline(phase) = definition.target else {
        return Err(format!("{} is not a hook", definition.title));
    };
    let with = serde_json::from_str::<serde_json::Value>(definition.example)
        .map_err(|error| format!("invalid catalog example for {}: {error}", definition.name))?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("catalog example for {} is not an object", definition.name))?;
    Ok(PipelineEntry {
        phase,
        uses: definition.reference.to_string(),
        with,
        enabled: true,
    })
}

fn begin_assertion_edit(d: &mut V1EditorState, index: usize) -> Result<(), String> {
    let assertion = d
        .assertions
        .assertions
        .get(index)
        .cloned()
        .ok_or_else(|| "the assertion no longer exists".to_string())?;
    if let Some(definition) = assertion
        .uses
        .strip_prefix("builtin:")
        .and_then(|reference| reference.split('@').next())
        .and_then(find_builtin)
    {
        select_builtin(d, definition);
        let parameters = definition
            .parameters
            .iter()
            .map(ParameterDefinition::builtin)
            .collect::<Vec<_>>();
        load_catalog_inputs(d, &assertion.with, &parameters);
    } else {
        let asset = d
            .index
            .as_ref()
            .and_then(|index| {
                index.assets.iter().find(|asset| {
                    asset.kind == AssetKind::Assertion
                        && asset_reference(asset) == assertion.uses
                        && asset.metadata.is_some()
                })
            })
            .cloned()
            .ok_or_else(|| format!("no catalog metadata found for {}", assertion.uses))?;
        let parameters = asset
            .metadata
            .as_ref()
            .ok_or_else(|| format!("no catalog metadata found for {}", assertion.uses))?
            .parameters
            .iter()
            .map(ParameterDefinition::project)
            .collect::<Vec<_>>();
        select_project_asset(d, &asset);
        load_catalog_inputs(d, &assertion.with, &parameters);
    }
    d.editing_assertion = Some(index);
    d.catalog_notice = Some("Editing configured assertion; save it in this form.".to_string());
    Ok(())
}

fn begin_hook_edit(d: &mut V1EditorState, index: usize) -> Result<(), String> {
    let hook = d
        .hooks
        .hooks
        .get(index)
        .cloned()
        .ok_or_else(|| "the hook no longer exists".to_string())?;
    if let Some(definition) = hook
        .uses
        .strip_prefix("builtin:")
        .and_then(|reference| reference.split('@').next())
        .and_then(find_builtin)
    {
        if !matches!(definition.target, BuiltinTarget::Pipeline(_))
            || definition.intent == BuiltinIntent::Validate
        {
            return Err(format!("{} is not a hook", definition.title));
        }
        select_builtin(d, definition);
        let parameters = definition
            .parameters
            .iter()
            .map(ParameterDefinition::builtin)
            .collect::<Vec<_>>();
        load_catalog_inputs(d, &hook.with, &parameters);
    } else {
        let asset = d
            .index
            .as_ref()
            .and_then(|index| {
                index.assets.iter().find(|asset| {
                    matches!(asset.kind, AssetKind::Hook | AssetKind::Extractor)
                        && asset_reference(asset) == hook.uses
                        && asset.metadata.is_some()
                })
            })
            .cloned()
            .ok_or_else(|| format!("no catalog metadata found for {}", hook.uses))?;
        let parameters = asset
            .metadata
            .as_ref()
            .ok_or_else(|| format!("no catalog metadata found for {}", hook.uses))?
            .parameters
            .iter()
            .map(ParameterDefinition::project)
            .collect::<Vec<_>>();
        select_project_asset(d, &asset);
        load_catalog_inputs(d, &hook.with, &parameters);
    }
    d.editing_hook = Some(index);
    d.catalog_notice = Some("Editing configured hook; save it in this form.".to_string());
    Ok(())
}

fn load_catalog_inputs(
    d: &mut V1EditorState,
    with: &serde_json::Map<String, serde_json::Value>,
    parameters: &[ParameterDefinition],
) {
    d.catalog_inputs.clear();
    for parameter in parameters {
        d.catalog_inputs.insert(
            parameter.name.clone(),
            with.get(&parameter.name)
                .map(|value| parameter_input_from_value(parameter.kind, value))
                .unwrap_or_default(),
        );
    }
}

fn asset_reference(asset: &AssetEntry) -> &str {
    asset
        .alias
        .as_deref()
        .or(asset.prefix_ref.as_deref())
        .unwrap_or(&asset.rel_path)
}

fn catalog_entry_title(reference: &str) -> &str {
    reference
        .strip_prefix("builtin:")
        .and_then(|reference| reference.split('@').next())
        .and_then(find_builtin)
        .map(|definition| definition.title)
        .unwrap_or(reference)
}

fn runtime_pane(ui: &mut egui::Ui, d: &V1EditorState) {
    match selected_result(d) {
        Some(r) if !r.runtime.is_empty() => {
            for (k, v) in &r.runtime {
                ui.label(RichText::new(format!("{k} = {v}")).monospace());
            }
        }
        Some(_) => {
            ui.weak("No runtime values extracted.");
        }
        None => {
            ui.weak("No run yet.");
        }
    }
}

fn diagnostics_pane(ui: &mut egui::Ui, d: &V1EditorState) {
    // Validate/parse messages first, then the run's diagnostics.
    for msg in &d.diagnostics {
        ui.colored_label(ui.visuals().error_fg_color, msg);
    }
    if let Some(r) = selected_result(d) {
        for diag in &r.diagnostics {
            let color = if diag.severity == forge_core::reqv1::Severity::Error {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().warn_fg_color
            };
            ui.colored_label(color, format!("[{}] {}", diag.code, diag.message));
        }
    }
    if d.diagnostics.is_empty()
        && selected_result(d)
            .map(|r| r.diagnostics.is_empty())
            .unwrap_or(true)
    {
        ui.weak("No diagnostics.");
    }
}

fn validate_now(d: &mut V1EditorState) {
    d.results.clear();
    d.selected_result = 0;
    d.diagnostics.clear();
    d.result_tab = ResultTab::Diagnostics;
    let (Some(root), Some(file)) = (d.root.clone(), d.file.clone().or_else(|| d.root.clone()))
    else {
        d.diagnostics = vec!["no project root".to_string()];
        return;
    };
    match effective_document(d) {
        Ok(doc) => {
            let permissive = |_n: &str| Some("<secret>".to_string());
            let env =
                forge_core::reqv1::load_request_environment(&root, &file, d.env_name.as_deref())
                    .unwrap_or(serde_json::Value::Null);
            match forge_core::reqv1::validate(&doc, &root, &file, env, &permissive) {
                Ok(ir) => {
                    d.diagnostics = vec![format!("ok — {} {}", ir.method, ir.url)];
                    if let Some(spec) = &d.openapi {
                        match spec.find_operation(doc.request.method, &doc.request.url) {
                            Some(operation) => d.diagnostics.extend(
                                openapi_request_issues(&doc, operation)
                                    .into_iter()
                                    .map(|issue| format!("[openapi] {issue}")),
                            ),
                            None => d
                                .diagnostics
                                .push("[openapi] request does not match an operation".to_string()),
                        }
                    }
                }
                Err(diags) => {
                    d.diagnostics = diags
                        .iter()
                        .map(|x| {
                            format!(
                                "[{}] {} {}",
                                x.code,
                                x.instance_path.clone().unwrap_or_default(),
                                x.message
                            )
                        })
                        .collect();
                }
            }
        }
        Err(e) => d.diagnostics = vec![format!("invalid JSON: {e}")],
    }
}

fn save_now(d: &mut V1EditorState) -> bool {
    let Some(path) = d.file.clone() else {
        d.diagnostics = vec!["no project request path".to_string()];
        return false;
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            d.diagnostics = vec![format!("failed to create {}: {error}", parent.display())];
            return false;
        }
    }
    if let Ok(mut document) = forge_core::reqv1::RequestDocument::parse(&d.text) {
        let inline = AssertionDocument::take_from_request(&mut document);
        let inline_hooks = HookDocument::take_from_request(&mut document);
        let mut extracted = false;
        if !inline.assertions.is_empty() {
            d.assertions.extend(inline);
            extracted = true;
        }
        if !inline_hooks.hooks.is_empty() {
            d.hooks.extend(inline_hooks);
            extracted = true;
        }
        if extracted {
            match serialize_request(&document) {
                Ok(text) => d.text = text,
                Err(error) => {
                    d.diagnostics = vec![error];
                    return false;
                }
            }
        }
    }
    let write = if d.new_file {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, d.text.as_bytes()))
    } else {
        std::fs::write(&path, &d.text)
    };
    match write {
        Ok(()) => {
            d.file = Some(path);
            d.new_file = false;
            if let Err(error) = d
                .assertions
                .save_for_request(d.file.as_deref().expect("file set above"))
                .and_then(|()| {
                    d.hooks
                        .save_for_request(d.file.as_deref().expect("file set above"))
                })
            {
                d.diagnostics = vec![error];
                return false;
            }
            d.dirty = false;
            validate_editor_json(d);
            d.diagnostics = d.load_index().err().into_iter().collect();
            true
        }
        Err(error) => {
            d.diagnostics = vec![format!("failed to save {}: {error}", path.display())];
            false
        }
    }
}

fn run_now(d: &mut V1EditorState, bridge: &Bridge) {
    let Some(root) = d.root.clone() else { return };
    // Run needs a file path for relative refs; use the saved file, or a
    // temp path under root for an unsaved buffer.
    let file = d
        .file
        .clone()
        .unwrap_or_else(|| root.join("__unsaved__.request.json"));
    let text = match effective_document(d).and_then(|document| serialize_request(&document)) {
        Ok(text) => text,
        Err(error) => {
            d.diagnostics = vec![format!("invalid JSON: {error}")];
            d.result_tab = ResultTab::Diagnostics;
            return;
        }
    };
    let run_id = d.next_run_id;
    d.next_run_id += 1;
    d.active_run = Some(run_id);
    d.in_flight = true;
    d.results.clear();
    d.selected_result = 0;
    d.last_response = None;
    d.diagnostics.clear();
    d.result_tab = ResultTab::Result;
    if let Err(error) = bridge.send(Cmd::RunV1 {
        run_id,
        root,
        file,
        text,
        env_name: d.env_name.clone(),
        mock: d.mock,
        allow_project_code: d.allow_project_code,
    }) {
        d.active_run = None;
        d.in_flight = false;
        d.diagnostics = vec![error];
        d.result_tab = ResultTab::Diagnostics;
    }
}

fn effective_document(d: &V1EditorState) -> Result<forge_core::reqv1::RequestDocument, String> {
    let mut document =
        forge_core::reqv1::RequestDocument::parse(&d.text).map_err(|error| error.to_string())?;
    let mut assertions = d.assertions.clone();
    assertions.extend(AssertionDocument::take_from_request(&mut document));
    let mut hooks = d.hooks.clone();
    hooks.extend(HookDocument::take_from_request(&mut document));
    hooks.apply_to(&mut document);
    assertions.apply_to(&mut document);
    Ok(document)
}

fn run_sequence_now(d: &mut V1EditorState, bridge: &Bridge) {
    let Some(root) = d.root.clone() else { return };
    let Some(sequence_file) = rfd::FileDialog::new()
        .set_directory(&root)
        .add_filter("sequence", &["json"])
        .pick_file()
    else {
        return;
    };
    let files = std::fs::read_to_string(&sequence_file)
        .map_err(|error| format!("cannot read {}: {error}", sequence_file.display()))
        .and_then(|text| {
            forge_core::reqv1::SequenceDocument::parse(&text)
                .map_err(|error| format!("invalid sequence: {error}"))
        })
        .and_then(|sequence| sequence.resolve_files(&root));
    let files = match files {
        Ok(files) => files,
        Err(error) => {
            d.diagnostics = vec![error];
            d.result_tab = ResultTab::Diagnostics;
            return;
        }
    };
    let env = d.env_name.clone();
    d.run_sequence(root, files, env, bridge);
}

fn document_has_matrix(text: &str) -> bool {
    forge_core::reqv1::RequestDocument::parse(text)
        .is_ok_and(|document| !document.matrix.is_empty())
}

fn selected_item(d: &V1EditorState) -> Option<&V1RunItem> {
    d.results.get(d.selected_result)
}

fn selected_result(d: &V1EditorState) -> Option<&RunResult> {
    selected_item(d).map(|item| &item.result)
}

// ---------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------

fn discover_openapi(
    root: &std::path::Path,
) -> (Option<PathBuf>, Option<ParsedSpec>, Option<String>) {
    let mut candidates = [
        "openapi.json",
        "openapi.yaml",
        "openapi.yml",
        "swagger.json",
        "swagger.yaml",
        "swagger.yml",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .filter(|path| path.is_file())
    .collect::<Vec<_>>();

    let specs = root.join("specs");
    let mut pending = specs
        .is_dir()
        .then_some(specs)
        .into_iter()
        .collect::<Vec<_>>();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "json" | "yaml" | "yml"
                    )
                })
            {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup();

    let mut error = None;
    for path in candidates {
        let parsed = std::fs::read_to_string(&path)
            .map_err(|cause| cause.to_string())
            .and_then(|text| {
                forge_core::openapi::parse_spec(&text).map_err(|cause| cause.to_string())
            });
        match parsed {
            Ok(spec) => return (Some(path), Some(spec), None),
            Err(cause) if error.is_none() => {
                error = Some((path, cause));
            }
            Err(_) => {}
        }
    }
    match error {
        Some((path, cause)) => (
            Some(path.clone()),
            None,
            Some(format!("invalid OpenAPI spec {}: {cause}", path.display())),
        ),
        None => (None, None, None),
    }
}

fn project_root_of(file: &std::path::Path) -> PathBuf {
    let mut dir = file.parent().map(std::path::Path::to_path_buf);
    while let Some(d) = dir {
        if d.join("project.json").exists() {
            return d;
        }
        dir = d.parent().map(std::path::Path::to_path_buf);
    }
    file.parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf()
}

fn escape_ptr(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn short(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    if s.len() > 30 {
        format!("{}…", &s[..29])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_columns_never_cross_the_sidebar_boundary() {
        for available_width in [360.0_f32, 480.0, 539.0, 540.0, 900.0] {
            let (catalog_width, request_width) = editor_column_widths(available_width);

            assert!(
                catalog_width + EDITOR_COLUMN_GAP + request_width <= available_width,
                "columns exceed {available_width}px: catalog={catalog_width}, request={request_width}"
            );
            assert!(
                request_width > TOOLBAR_MENU_CELL_WIDTH + TOOLBAR_TRAILING_GUTTER,
                "toolbar edge controls do not fit inside {request_width}px"
            );
        }
    }

    #[test]
    fn editor_font_zoom_is_smooth_and_uses_setting_bounds() {
        let zoomed = zoom_editor_font_size(15.0, 1.01);
        assert!(zoomed > 15.0 && zoomed < 16.0);
        assert_eq!(zoom_editor_font_size(24.0, 2.0), 24.0);
        assert_eq!(zoom_editor_font_size(9.0, 0.5), 9.0);
    }

    #[test]
    fn matched_openapi_assist_only_reserves_its_compact_row() {
        let spec = openapi_fixture();
        let text = apply_openapi_operation(SKELETON, &spec.operations[0]).unwrap();
        let editor = V1EditorState {
            validated_document: forge_core::reqv1::RequestDocument::parse(&text).ok(),
            openapi: Some(spec),
            ..V1EditorState::default()
        };

        assert_eq!(request_editor_footer_height(&editor), 28.0);
    }

    #[test]
    fn openapi_suggestions_reserve_two_assist_rows() {
        let spec = openapi_fixture();
        let document = forge_core::reqv1::RequestDocument::parse(SKELETON).unwrap();
        let editor = V1EditorState {
            validated_document: Some(document),
            openapi: Some(spec),
            ..V1EditorState::default()
        };

        assert_eq!(request_editor_footer_height(&editor), 64.0);
    }

    #[test]
    fn results_typography_scales_without_changing_the_default() {
        let mut style = egui::Style::default();
        let body_size = style.text_styles[&egui::TextStyle::Body].size;

        scale_results_typography(&mut style, crate::state::DEFAULT_EDITOR_FONT_SIZE);
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, body_size);

        scale_results_typography(&mut style, crate::state::DEFAULT_EDITOR_FONT_SIZE * 1.2);
        assert!((style.text_styles[&egui::TextStyle::Body].size - body_size * 1.2).abs() < 0.01);
    }
    use forge_core::reqv1::index::{AssetEntry, Usage};
    use forge_core::reqv1::AssetKind;

    fn asset(kind: AssetKind, alias: &str) -> AssetEntry {
        AssetEntry {
            path: String::new(),
            rel_path: "assets/x".to_string(),
            kind,
            alias: Some(alias.to_string()),
            prefix_ref: None,
            used_by: Vec::<Usage>::new(),
            data: None,
            metadata: None,
        }
    }

    fn openapi_fixture() -> ParsedSpec {
        forge_core::openapi::parse_spec(
            &serde_json::json!({
                "openapi": "3.0.3",
                "info": {"title": "Shop", "version": "1"},
                "paths": {
                    "/pets/{petId}": {
                        "post": {
                            "operationId": "updatePet",
                            "parameters": [
                                {"name": "petId", "in": "path", "required": true, "schema": {"type": "string"}},
                                {"name": "expand", "in": "query", "required": true, "schema": {"type": "string"}},
                                {"name": "X-Tenant", "in": "header", "required": true, "schema": {"type": "string"}}
                            ],
                            "requestBody": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "required": ["name"],
                                            "properties": {"name": {"type": "string"}}
                                        }
                                    }
                                }
                            },
                            "responses": {
                                "200": {
                                    "description": "ok",
                                    "content": {
                                        "application/json": {
                                            "schema": {
                                                "type": "object",
                                                "required": ["name"],
                                                "properties": {"name": {"type": "string"}}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn openapi_completion_fills_path_parameters_and_body() {
        let spec = openapi_fixture();
        let operation = &spec.operations[0];
        let completed = apply_openapi_operation(SKELETON, operation).unwrap();
        let document = forge_core::reqv1::RequestDocument::parse(&completed).unwrap();

        assert_eq!(
            document.request.url,
            "https://example.com/pets/${bindings.petId}"
        );
        assert!(document.bindings.contains_key("petId"));
        assert!(document
            .request
            .query
            .iter()
            .any(|parameter| parameter.name == "expand"));
        assert!(document
            .request
            .headers
            .iter()
            .any(|header| header.name == "X-Tenant"));
        assert!(document.request.body.is_some());
        assert!(openapi_request_issues(&document, operation).is_empty());
    }

    #[test]
    fn openapi_operation_filters_combine_method_shape_and_text() {
        let spec = openapi_fixture();
        let operation = &spec.operations[0];

        assert!(OpenApiFilter::Method(Method::Post).matches(operation));
        assert!(!OpenApiFilter::Method(Method::Get).matches(operation));
        assert!(OpenApiFilter::Headers.matches(operation));
        assert!(OpenApiFilter::Query.matches(operation));
        assert!(OpenApiFilter::Path.matches(operation));
        assert!(OpenApiFilter::Body.matches(operation));
        assert!(operation_matches_query(operation, "updatepet"));
        assert!(!operation_matches_query(operation, "missing"));
    }

    #[test]
    fn local_openapi_specs_are_discovered_without_project_config() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("specs")).unwrap();
        std::fs::write(
            root.path().join("specs/shop.json"),
            serde_json::to_string(&openapi_fixture().raw).unwrap(),
        )
        .unwrap();

        let (source, spec, error) = discover_openapi(root.path());
        assert!(source.unwrap().ends_with("specs/shop.json"));
        assert_eq!(spec.unwrap().title, "Shop");
        assert!(error.is_none());
    }

    #[test]
    fn response_contract_issues_use_the_discovered_operation() {
        let spec = openapi_fixture();
        let operation = &spec.operations[0];
        let text = apply_openapi_operation(SKELETON, operation).unwrap();
        let editor = V1EditorState {
            text,
            openapi: Some(spec),
            ..V1EditorState::default()
        };
        let response = ResponseView {
            status: 200,
            headers: vec![(
                "Content-Type".to_string(),
                "application/json; charset=utf-8".to_string(),
            )],
            body: br#"{"name":"Fido"}"#.to_vec(),
            time_ms: 12,
        };

        assert!(openapi_response_issues(&editor, &response).is_empty());
    }

    #[test]
    fn snippet_shapes_per_kind() {
        assert!(
            snippet_for(&asset(AssetKind::Data, "data:users"), "data:users").contains("\"ref\"")
        );
        assert!(snippet_for(
            &asset(AssetKind::Hook, "project:hooks/x"),
            "project:hooks/x"
        )
        .contains("\"phase\": \"beforeRequest\""));
        assert!(snippet_for(
            &asset(AssetKind::Assertion, "project:assertions/x"),
            "project:assertions/x"
        )
        .contains("afterResponse"));
    }

    #[test]
    fn project_metadata_builds_a_typed_configured_snippet() {
        let mut asset = asset(AssetKind::Assertion, "project:assertions/user");
        let metadata = ProjectAssetMetadata {
            title: "User".to_string(),
            description: String::new(),
            intent: BuiltinIntent::Validate,
            phase: Some(forge_core::reqv1::model::PipelinePhase::AfterResponse),
            parameters: vec![ProjectAssetParameter {
                name: "expected".to_string(),
                label: "Expected".to_string(),
                kind: BuiltinParameterKind::Integer,
                required: true,
                default: None,
                options: Vec::new(),
                example: "201".to_string(),
            }],
            example: serde_json::json!({"expected": 201}),
        };
        asset.metadata = Some(metadata.clone());
        let parameters = metadata
            .parameters
            .iter()
            .map(ParameterDefinition::project)
            .collect::<Vec<_>>();
        let inputs = BTreeMap::from([(
            "expected".to_string(),
            ParameterInput {
                source: ParameterSource::Literal,
                value: "201".to_string(),
            },
        )]);

        let snippet = project_snippet(
            &asset,
            "project:assertions/user",
            &metadata,
            &parameters,
            &inputs,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&snippet).unwrap();

        assert_eq!(value["with"]["expected"], 201);
        assert_eq!(value["phase"], "afterResponse");
    }

    #[test]
    fn matrix_documents_get_a_distinct_run_action() {
        assert!(!document_has_matrix(SKELETON));
        let matrix = SKELETON.replace(
            "\"request\": {",
            "\"matrix\": {\"case\": {\"value\": [1, 2]}},\n  \"request\": {",
        );
        assert!(document_has_matrix(&matrix));
    }

    #[test]
    fn structured_insert_uses_named_slots_instead_of_cursor_text() {
        let insert = PendingInsert {
            target: InsertTarget::Binding,
            suggested_name: "user-id".to_string(),
            snippet: r#"{"ref":"data:users#/0/id"}"#.to_string(),
        };
        let (text, notice) = apply_structured_insert(SKELETON, insert).unwrap();
        let document = forge_core::reqv1::RequestDocument::parse(&text).unwrap();

        assert!(document.bindings.contains_key("user_id"));
        assert!(notice.contains("${bindings.user_id}"));
    }

    #[test]
    fn parameter_sources_stay_whole_string_expressions() {
        assert_eq!(
            sourced_value(ParameterSource::Binding, "user.id"),
            Some(serde_json::json!("${bindings.user.id}"))
        );
        assert_eq!(
            sourced_value(ParameterSource::Environment, "baseUrl"),
            Some(serde_json::json!("${env.baseUrl}"))
        );
        assert_eq!(
            sourced_value(ParameterSource::Runtime, "token"),
            Some(serde_json::json!("${runtime.token}"))
        );
        assert_eq!(
            sourced_value(ParameterSource::Matrix, "region"),
            Some(serde_json::json!("${matrix.region}"))
        );
        assert_eq!(
            sourced_value(ParameterSource::Secret, "apiKey"),
            Some(serde_json::json!("${secret.apiKey}"))
        );
        assert_eq!(sourced_value(ParameterSource::Literal, "200"), None);
    }

    #[test]
    fn typed_parameters_hide_string_only_secret_source() {
        assert!(parameter_sources(BuiltinParameterKind::String).contains(&ParameterSource::Secret));
        for kind in [
            BuiltinParameterKind::Integer,
            BuiltinParameterKind::Boolean,
            BuiltinParameterKind::Json,
        ] {
            let sources = parameter_sources(kind);
            assert!(!sources.contains(&ParameterSource::Secret));
            assert!(sources.contains(&ParameterSource::Binding));
            assert!(sources.contains(&ParameterSource::Environment));
            assert!(sources.contains(&ParameterSource::Runtime));
            assert!(sources.contains(&ParameterSource::Matrix));
        }
    }

    #[test]
    fn secret_suggestions_expose_names_without_values() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env.local"), "API_TOKEN=super-secret\n").unwrap();
        let editor = V1EditorState {
            root: Some(root.path().to_path_buf()),
            text: SKELETON.to_string(),
            ..V1EditorState::default()
        };

        let suggestions = parameter_suggestions(&editor, ParameterSource::Secret);

        assert_eq!(suggestions, ["API_TOKEN"]);
        assert!(!suggestions.iter().any(|value| value == "super-secret"));
    }

    #[test]
    fn preview_rejects_scopes_the_bridge_cannot_supply() {
        let mut inputs = BTreeMap::from([(
            "expected".to_string(),
            ParameterInput {
                source: ParameterSource::Binding,
                value: "status".to_string(),
            },
        )]);
        assert!(preview_supports_sources(&inputs));

        inputs.get_mut("expected").unwrap().source = ParameterSource::Matrix;
        assert!(!preview_supports_sources(&inputs));
        inputs.get_mut("expected").unwrap().source = ParameterSource::Runtime;
        assert!(!preview_supports_sources(&inputs));
    }

    #[test]
    fn configured_builtin_inserts_typed_or_sourced_parameters() {
        let definition = find_builtin("assert-status").unwrap();
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "expected".to_string(),
            ParameterInput {
                source: ParameterSource::Literal,
                value: "201".to_string(),
            },
        );
        let literal: serde_json::Value =
            serde_json::from_str(&builtin_snippet(definition, &inputs).unwrap()).unwrap();
        assert_eq!(literal["with"]["expected"], serde_json::json!(201));

        inputs.get_mut("expected").unwrap().value = "-1".to_string();
        assert!(builtin_snippet(definition, &inputs)
            .unwrap_err()
            .contains("non-negative integer"));

        inputs.get_mut("expected").unwrap().source = ParameterSource::Binding;
        inputs.get_mut("expected").unwrap().value = "expectedStatus".to_string();
        let sourced: serde_json::Value =
            serde_json::from_str(&builtin_snippet(definition, &inputs).unwrap()).unwrap();
        assert_eq!(
            sourced["with"]["expected"],
            serde_json::json!("${bindings.expectedStatus}")
        );
        assert_eq!(sourced["use"], "builtin:assert-status@1");
        assert_eq!(sourced["phase"], "afterResponse");
    }

    #[test]
    fn stale_preview_result_is_ignored() {
        let mut editor = V1EditorState {
            active_preview: Some(2),
            preview_in_flight: true,
            ..V1EditorState::default()
        };
        let preview = || CatalogPreview {
            request_before: None,
            request_after: None,
            assertions: Vec::new(),
            runtime_writes: BTreeMap::new(),
            logs: Vec::new(),
            diagnostics: Vec::new(),
        };

        editor.handle_preview(1, Ok(preview()));
        assert!(editor.preview.is_none());
        assert!(editor.preview_in_flight);
        assert_eq!(editor.active_preview, Some(2));

        editor.handle_preview(2, Ok(preview()));
        assert!(editor.preview.is_some());
        assert!(!editor.preview_in_flight);
        assert_eq!(editor.active_preview, None);
    }

    #[test]
    fn opening_document_clears_stale_run_state() {
        let root = std::env::temp_dir().join(format!(
            "forge-v1-editor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("request.json");
        std::fs::write(&file, SKELETON).unwrap();

        let mut editor = V1EditorState {
            active_run: Some(7),
            in_flight: true,
            ..V1EditorState::default()
        };
        editor.open_new(root.clone(), None);
        assert_eq!(editor.active_run, None);
        assert!(!editor.in_flight);

        editor.active_run = Some(8);
        editor.in_flight = true;
        editor.open_file(file, None).unwrap();
        assert_eq!(editor.active_run, None);
        assert!(!editor.in_flight);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_requests_get_a_derived_collision_free_path() {
        let root = tempfile::tempdir().unwrap();
        let requests = root.path().join("requests");
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(requests.join("new.request.json"), "{}").unwrap();
        let mut editor = V1EditorState::default();

        editor.open_new(root.path().to_path_buf(), None);
        assert_eq!(
            editor.file.as_deref(),
            Some(requests.join("new-2.request.json").as_path())
        );

        save_now(&mut editor);
        assert!(requests.join("new-2.request.json").is_file());
    }

    #[test]
    fn new_requests_use_the_selected_story_folder() {
        let root = tempfile::tempdir().unwrap();
        let story = root.path().join("requests/checkout");
        std::fs::create_dir_all(&story).unwrap();
        let mut editor = V1EditorState::default();

        editor.open_new_in(root.path().to_path_buf(), story.clone(), None);
        assert_eq!(
            editor.file.as_deref(),
            Some(story.join("new.request.json").as_path())
        );
    }

    #[test]
    fn saving_a_new_request_never_overwrites_a_racing_file() {
        let root = tempfile::tempdir().unwrap();
        let mut editor = V1EditorState::default();
        editor.open_new(root.path().to_path_buf(), None);
        let path = editor.file.clone().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "keep").unwrap();

        save_now(&mut editor);

        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
        assert!(editor.diagnostics[0].contains("failed to save"));
    }

    #[test]
    fn default_request_validates_without_environment_config() {
        let root = tempfile::tempdir().unwrap();
        forge_core::store::Workspace::create(root.path(), "Zero config").unwrap();
        let document = forge_core::reqv1::RequestDocument::parse(SKELETON).unwrap();

        let result = forge_core::reqv1::validate(
            &document,
            root.path(),
            &root.path().join("requests/new.request.json"),
            serde_json::json!({}),
            &|_| None,
        );

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn assertion_insert_is_saved_beside_the_request() {
        let root = tempfile::tempdir().unwrap();
        let mut editor = V1EditorState::default();
        editor.open_new(root.path().to_path_buf(), None);
        let request_path = editor.file.clone().unwrap();
        let insert = PendingInsert {
            target: InsertTarget::Assertion,
            suggested_name: "assert-status".to_string(),
            snippet: r#"{"phase":"afterResponse","use":"builtin:assert-status@1","with":{"expected":201}}"#
                .to_string(),
        };

        apply_insert(&mut editor, insert).unwrap();
        assert!(save_now(&mut editor));

        let request = forge_core::reqv1::RequestDocument::parse(
            &std::fs::read_to_string(&request_path).unwrap(),
        )
        .unwrap();
        let assertions = AssertionDocument::load_for_request(&request_path).unwrap();
        assert!(request.pipeline.is_empty());
        assert_eq!(assertions.assertions.len(), 1);
        assert_eq!(effective_document(&editor).unwrap().pipeline.len(), 1);
    }

    #[test]
    fn configured_assertion_can_be_edited_in_the_catalog() {
        let mut editor = V1EditorState {
            catalog_view: CatalogView::Project,
            ..V1EditorState::default()
        };
        editor.assertions.push(AssertionEntry {
            uses: "builtin:assert-status@1".to_string(),
            with: serde_json::Map::from_iter([(
                "expected".to_string(),
                "${env.expectedStatus}".into(),
            )]),
            enabled: true,
        });

        begin_assertion_edit(&mut editor, 0).unwrap();
        assert_eq!(editor.catalog_view, CatalogView::Builtins);
        assert_eq!(editor.selected_builtin.as_deref(), Some("assert-status"));
        assert_eq!(
            editor.catalog_inputs["expected"].source,
            ParameterSource::Environment
        );
        assert_eq!(editor.catalog_inputs["expected"].value, "expectedStatus");

        let expected = editor.catalog_inputs.get_mut("expected").unwrap();
        expected.source = ParameterSource::Literal;
        expected.value = "204".to_string();
        let definition = find_builtin("assert-status").unwrap();
        let insert = PendingInsert {
            target: InsertTarget::Assertion,
            suggested_name: definition.name.to_string(),
            snippet: builtin_snippet(definition, &editor.catalog_inputs).unwrap(),
        };
        apply_insert(&mut editor, insert).unwrap();

        assert_eq!(editor.assertions.assertions.len(), 1);
        assert_eq!(editor.assertions.assertions[0].with["expected"], 204);
        assert_eq!(editor.editing_assertion, None);
    }

    #[test]
    fn catalog_search_and_intent_filters_combine() {
        let status = find_builtin("assert-status").unwrap();
        let bearer = find_builtin("bearer").unwrap();

        assert!(builtin_matches(
            status,
            "http response status",
            Some("Validate")
        ));
        assert!(!builtin_matches(
            status,
            "http response status",
            Some("Prepare")
        ));
        assert!(builtin_matches(bearer, "authorization", None));
        assert!(!builtin_matches(bearer, "response cookie", None));
    }

    #[test]
    fn hook_is_saved_beside_the_request_and_edited_in_the_catalog() {
        let root = tempfile::tempdir().unwrap();
        let mut editor = V1EditorState::default();
        editor.open_new(root.path().to_path_buf(), None);
        let request_path = editor.file.clone().unwrap();
        let insert = PendingInsert {
            target: InsertTarget::Pipeline,
            suggested_name: "header".to_string(),
            snippet: r#"{"phase":"beforeRequest","use":"builtin:header@1","with":{"name":"X-Test","value":"old"}}"#
                .to_string(),
        };
        apply_insert(&mut editor, insert).unwrap();

        begin_hook_edit(&mut editor, 0).unwrap();
        assert_eq!(editor.selected_builtin.as_deref(), Some("header"));
        editor.catalog_inputs.get_mut("value").unwrap().value = "new".to_string();
        let definition = find_builtin("header").unwrap();
        let update = PendingInsert {
            target: InsertTarget::Pipeline,
            suggested_name: definition.name.to_string(),
            snippet: builtin_snippet(definition, &editor.catalog_inputs).unwrap(),
        };
        apply_insert(&mut editor, update).unwrap();
        assert!(save_now(&mut editor));

        let request = forge_core::reqv1::RequestDocument::parse(
            &std::fs::read_to_string(&request_path).unwrap(),
        )
        .unwrap();
        let hooks = HookDocument::load_for_request(&request_path).unwrap();
        assert!(request.pipeline.is_empty());
        assert_eq!(hooks.hooks.len(), 1);
        assert_eq!(hooks.hooks[0].with["value"], "new");
        assert_eq!(effective_document(&editor).unwrap().pipeline.len(), 1);
    }

    #[test]
    fn html_response_markup_is_indented() {
        let html = "<!doctype html><html lang=\"en\"><head><title>Example Domain</title><link rel=\"icon\" href=\"data:,\"></head><body><h1>Example Domain</h1></body></html>";

        let formatted = pretty_markup(html);

        assert!(formatted.contains("\n<html lang=\"en\">\n  <head>"));
        assert!(formatted.contains("\n      Example Domain\n    </title>"));
        assert!(formatted.contains("\n    <link rel=\"icon\" href=\"data:,\">\n  </head>"));
        assert!(formatted.ends_with("\n</html>"));
    }

    #[test]
    fn failed_run_opens_diagnostics() {
        let mut editor = V1EditorState {
            active_run: Some(7),
            in_flight: true,
            ..V1EditorState::default()
        };

        editor.handle_result(7, Err("project code is disabled".to_string()));

        assert_eq!(editor.result_tab, ResultTab::Diagnostics);
        assert_eq!(editor.diagnostics, ["project code is disabled"]);
    }

    #[test]
    fn failed_open_keeps_the_current_buffer() {
        let mut editor = V1EditorState {
            text: "keep me".to_string(),
            dirty: true,
            ..V1EditorState::default()
        };

        let error = editor
            .open_file(
                std::path::PathBuf::from("/definitely/missing/request.json"),
                None,
            )
            .expect_err("missing file must fail");

        assert!(error.contains("failed to read"));
        assert_eq!(editor.text, "keep me");
        assert!(editor.dirty);
    }

    #[test]
    fn auth_fetcher_is_saved_centrally_not_in_the_request() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("requests/auth/token.request.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(root.path().join("project.json"), "{\"formatVersion\":1}").unwrap();
        std::fs::write(&file, SKELETON).unwrap();
        let mut editor = V1EditorState {
            root: Some(root.path().to_path_buf()),
            file: Some(file.clone()),
            project_auth: Some(ProjectAuthConfig::for_request(
                "requests/auth/token.request.json".to_string(),
            )),
            auth_dirty: true,
            ..V1EditorState::default()
        };

        save_project_auth(&mut editor);

        let project = forge_core::reqv1::load_project(root.path()).unwrap();
        assert_eq!(
            project.auth.unwrap().request,
            "requests/auth/token.request.json"
        );
        let request =
            forge_core::reqv1::RequestDocument::parse(&std::fs::read_to_string(file).unwrap())
                .unwrap();
        assert_eq!(request.meta.id, "new.request");
        assert!(!editor.auth_dirty);
    }

    #[test]
    fn provider_presets_build_expected_oauth_requests() {
        let cases = [
            (
                AuthProvider::Generic,
                "idp.example.com/token",
                "",
                "scope-a",
                "https://idp.example.com/token",
            ),
            (
                AuthProvider::Keycloak,
                "https://idp.example.com/auth",
                "acme",
                "scope-a",
                "https://idp.example.com/auth/realms/acme/protocol/openid-connect/token",
            ),
            (
                AuthProvider::Auth0,
                "tenant.auth0.com",
                "",
                "https://api.example.com",
                "https://tenant.auth0.com/oauth/token",
            ),
            (
                AuthProvider::Entra,
                "tenant-id",
                "",
                "https://api.example.com/.default",
                "https://login.microsoftonline.com/tenant-id/oauth2/v2.0/token",
            ),
        ];

        for (provider, endpoint, realm, scope, expected_url) in cases {
            let draft = AuthDraft {
                provider,
                endpoint: endpoint.to_string(),
                realm: realm.to_string(),
                client_id: "client-id".to_string(),
                client_secret: "must-not-be-persisted".to_string(),
                scope: scope.to_string(),
            };

            let document = provider_auth_document(&draft, provider.file_stem()).unwrap();
            let value = serde_json::to_value(document).unwrap();
            let form = &value["request"]["body"]["value"];

            assert_eq!(value["request"]["url"], expected_url);
            assert_eq!(form["grant_type"], "client_credentials");
            assert_eq!(form["client_id"], "client-id");
            assert_eq!(
                form["client_secret"],
                format!("${{secret.{}}}", provider.secret_name())
            );
            assert!(!value.to_string().contains("must-not-be-persisted"));
            if provider == AuthProvider::Auth0 {
                assert_eq!(form["audience"], scope);
            } else {
                assert_eq!(form["scope"], scope);
            }
        }
    }

    #[test]
    fn provider_setup_creates_and_activates_a_derived_auth_request() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("project.json"), "{\"formatVersion\":1}").unwrap();
        let mut editor = V1EditorState {
            root: Some(root.path().to_path_buf()),
            auth_draft: AuthDraft {
                provider: AuthProvider::Keycloak,
                endpoint: "https://sso.example.com".to_string(),
                realm: "acme".to_string(),
                client_id: "forge".to_string(),
                client_secret: " leading \"secret\" value ".to_string(),
                scope: String::new(),
            },
            ..V1EditorState::default()
        };

        create_provider_auth_request(&mut editor);

        let project = forge_core::reqv1::load_project(root.path()).unwrap();
        let auth = project.auth.unwrap();
        assert_eq!(auth.request, "requests/auth/keycloak-token.request.json");
        let request_text = std::fs::read_to_string(root.path().join(&auth.request)).unwrap();
        assert!(forge_core::reqv1::RequestDocument::parse(&request_text).is_ok());
        assert!(!request_text.contains("leading"));
        assert_eq!(
            forge_core::reqv1::load_file_secrets(root.path())
                .get("KEYCLOAK_CLIENT_SECRET")
                .map(String::as_str),
            Some(" leading \"secret\" value ")
        );
        assert!(editor.auth_draft.client_secret.is_empty());
        assert!(!editor.auth_dirty);
    }

    #[test]
    fn advisor_context_redacts_sensitive_values_and_headers() {
        let mut value = serde_json::json!({
            "token": "secret-token",
            "nested": {"password": "secret-password", "safe": "visible"},
            "headers": [
                {"name": "Authorization", "value": "Bearer secret"},
                {"name": "Accept", "value": "application/json"}
            ]
        });

        redact_sensitive_json(&mut value);

        assert_eq!(value["token"], "***");
        assert_eq!(value["nested"]["password"], "***");
        assert_eq!(value["nested"]["safe"], "visible");
        assert_eq!(value["headers"][0]["value"], "***");
        assert_eq!(value["headers"][1]["value"], "application/json");
        assert!(!value.to_string().contains("secret"));
    }

    #[test]
    fn stale_advisor_reply_does_not_replace_the_current_answer() {
        let mut editor = V1EditorState {
            active_advisor: Some(2),
            advisor_answer: Some("current".to_string()),
            ..V1EditorState::default()
        };

        editor.handle_advisor(1, Ok("stale".to_string()));

        assert_eq!(editor.active_advisor, Some(2));
        assert_eq!(editor.advisor_answer.as_deref(), Some("current"));
    }

    #[test]
    fn editor_validation_reports_exact_json_location() {
        let mut editor = V1EditorState {
            text: "{\n  \"formatVersion\": nope\n}".to_string(),
            ..V1EditorState::default()
        };

        validate_editor_json(&mut editor);

        let diagnostic = editor.json_diagnostic.unwrap();
        assert_eq!(diagnostic.line, 2);
        assert!(diagnostic.column > 2);
        assert!(editor.validated_document.is_none());
    }

    #[test]
    fn marked_openapi_operations_round_trip_locally() {
        let root = tempfile::tempdir().unwrap();
        let marked = BTreeSet::from(["get-pets".to_string(), "post-pets".to_string()]);

        save_marked_operations(root.path(), &marked).unwrap();

        assert_eq!(load_marked_operations(root.path()).unwrap(), marked);
    }
}
