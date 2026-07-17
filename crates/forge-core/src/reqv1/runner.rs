//! Runner: document → IR → beforeRequest hooks → HTTP send (or mock) →
//! afterResponse assertions/extractors → [`RunResult`]. See §9, §12, §17.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::exec::{HttpEngine, ResolvedBody as ExecBody, ResolvedRequest as ExecRequest};

use super::build::{build_ir, BuildInputs};
use super::diag::{Code, Diagnostic};
use super::ir::{ResolvedBody, ResolvedHeader, ResolvedMock, ResolvedRequest};
use super::model::{PipelinePhase, ProjectConfig, RequestDocument};
use super::pipeline::{
    is_project_asset, run_after_response, run_before_request, AssertionResult, ResponseView,
};
use super::refs::RefResolver;
use super::resolve::DataStore;

/// Outcome of one request run (or one matrix case). See §17.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub request_id: String,
    pub status: RunStatus,
    pub http: Option<HttpResultView>,
    pub assertions: Vec<AssertionResult>,
    pub runtime: BTreeMap<String, Value>,
    pub diagnostics: Vec<Diagnostic>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Passed,
    Failed,
    Error,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResultView {
    pub status: u16,
    pub time_ms: u64,
    pub bytes: usize,
}

/// How to source the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Send the request over HTTP.
    Http,
    /// Serve the document's `mock` instead of sending (§10).
    Mock,
}

/// Load `<root>/environments/<name>.json`, or an empty object if `name` is
/// None. Errors if a named environment is missing.
pub fn load_environment(root: &Path, name: Option<&str>) -> Result<Value, Diagnostic> {
    match name {
        None => Ok(Value::Object(Default::default())),
        Some(name) => {
            let path = root.join("environments").join(format!("{name}.json"));
            let text = std::fs::read_to_string(&path).map_err(|e| {
                Diagnostic::new(Code::AssetNotFound, format!("environment {name}: {e}"))
            })?;
            serde_json::from_str(&text).map_err(|e| {
                Diagnostic::new(Code::InvalidAssetInput, format!("environment {name}: {e}"))
            })
        }
    }
}

/// Load `<root>/project.json` (empty config if absent).
pub fn load_project(root: &Path) -> Result<ProjectConfig, Diagnostic> {
    let path = root.join("project.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Diagnostic::new(Code::InvalidAssetInput, format!("project.json: {e}"))),
        Err(_) => Ok(ProjectConfig::default()),
    }
}

/// Validate a request document down to the canonical IR without touching the
/// network (§12 stages 1–5). Returns the IR or the collected diagnostics.
pub fn validate(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &dyn Fn(&str) -> Option<String>,
) -> Result<ResolvedRequest, Vec<Diagnostic>> {
    validate_case(doc, root, request_file, env, secret, Value::Null, empty_object())
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// [`validate`] for one specific matrix case (`matrix` = the case object)
/// and incoming `runtime` (from earlier requests in a sequence).
pub fn validate_case(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &dyn Fn(&str) -> Option<String>,
    matrix: Value,
    runtime: Value,
) -> Result<ResolvedRequest, Vec<Diagnostic>> {
    let project = load_project(root).map_err(|d| vec![d])?;
    let resolver = RefResolver::new(root, &project).map_err(|e| e.0)?;
    let store = DataStore::new(&resolver);
    let base_dir = request_file.parent().unwrap_or(root);
    let inp = BuildInputs {
        resolver: &resolver,
        store: &store,
        base_dir,
        env,
        matrix,
        runtime,
        secret,
    };
    build_ir(doc, &inp).map_err(|e| e.0)
}

/// Full run: validate + execute. `secret` is the secret provider.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &dyn Fn(&str) -> Option<String>,
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
    matrix: Value,
) -> RunResult {
    run_with_runtime(doc, root, request_file, env, secret, engine, mode, cancel, matrix, empty_object()).await
}

/// [`run`] plus incoming runtime (`${runtime.*}` from earlier requests in a
/// sequence). Runs all four pipeline phases: beforeRequest → send/mock →
/// afterResponse, then onError (only if the run errored) and finally
/// (always). See §9.
#[allow(clippy::too_many_arguments)]
pub async fn run_with_runtime(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &dyn Fn(&str) -> Option<String>,
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
    matrix: Value,
    runtime_in: Value,
) -> RunResult {
    let started = std::time::Instant::now();

    let mut ir = match validate_case(doc, root, request_file, env, secret, matrix, runtime_in) {
        Ok(ir) => ir,
        Err(diags) => {
            return RunResult {
                request_id: doc.meta.id.clone(),
                status: RunStatus::Error,
                http: None,
                assertions: Vec::new(),
                runtime: BTreeMap::new(),
                diagnostics: diags,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut assertions: Vec<AssertionResult> = Vec::new();
    let mut runtime: BTreeMap<String, Value> = BTreeMap::new();
    let mut error: Option<String> = None;

    // --- beforeRequest hooks ---
    for entry in ir.pipeline.clone().iter().filter(|e| e.phase == PipelinePhase::BeforeRequest) {
        let outcome = if is_project_asset(entry) {
            run_js_hook(entry, &ir)
        } else {
            run_before_request(entry, &ir)
        };
        match outcome {
            Ok(patch) => apply_patch(&mut ir, patch.headers, patch.url, &mut diagnostics),
            Err(d) => {
                error.get_or_insert_with(|| d.message.clone());
                diagnostics.push(d);
            }
        }
    }

    // --- send or mock (skipped if a beforeRequest hook already errored) ---
    let response = if error.is_some() {
        None
    } else {
        match mode {
            RunMode::Mock => match &ir.mock.clone() {
                Some(m) => mock_response(m, &ir, &mut diagnostics),
                None => {
                    let d = Diagnostic::new(
                        Code::HttpError,
                        "mock mode requested but the document has no mock",
                    );
                    error = Some(d.message.clone());
                    diagnostics.push(d);
                    None
                }
            },
            RunMode::Http => match send(&ir, engine, cancel).await {
                Ok(r) => Some(r),
                Err(d) => {
                    error = Some(d.message.clone());
                    diagnostics.push(d);
                    None
                }
            },
        }
    };

    // --- afterResponse assertions + extractors (only with a response) ---
    if let Some(response) = &response {
        for entry in ir.pipeline.iter().filter(|e| e.phase == PipelinePhase::AfterResponse) {
            let outcome = reaction_asset(entry, &ir, Some(response), None);
            merge_reaction(outcome, entry, &mut assertions, &mut runtime, &mut diagnostics, &mut error);
        }
    }

    // --- onError: only when the run errored (§9) ---
    if error.is_some() {
        let err = error.clone();
        for entry in ir.pipeline.clone().iter().filter(|e| e.phase == PipelinePhase::OnError) {
            let outcome = reaction_asset(entry, &ir, response.as_ref(), err.as_deref());
            merge_reaction(outcome, entry, &mut assertions, &mut runtime, &mut diagnostics, &mut error);
        }
    }

    // --- finally: always (§9), for teardown/always-checks ---
    for entry in ir.pipeline.clone().iter().filter(|e| e.phase == PipelinePhase::Finally) {
        let outcome = reaction_asset(entry, &ir, response.as_ref(), error.as_deref());
        merge_reaction(outcome, entry, &mut assertions, &mut runtime, &mut diagnostics, &mut error);
    }

    let http = response.as_ref().map(|r| HttpResultView {
        status: r.status,
        time_ms: r.time_ms,
        bytes: r.body.len(),
    });
    let has_asset_error = diagnostics.iter().any(Diagnostic::is_error);
    let status = if has_asset_error || error.is_some() || response.is_none() {
        RunStatus::Error
    } else if assertions.iter().any(|a| !a.passed) {
        RunStatus::Failed
    } else {
        RunStatus::Passed
    };
    finish(&ir, status, http, assertions, runtime, diagnostics, started)
}

/// Run a sequence of request files in order, threading extracted runtime
/// forward: request N's `extract-*` results are visible to request N+1 as
/// `${runtime.*}` (§9). Each file runs with a fresh matrix (no matrix), but
/// the accumulated runtime persists. `root` is the shared project root.
#[allow(clippy::too_many_arguments)]
pub async fn run_sequence(
    files: &[std::path::PathBuf],
    root: &Path,
    env: Value,
    secret: &dyn Fn(&str) -> Option<String>,
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
) -> Vec<RunResult> {
    let mut runtime = empty_object();
    let mut results = Vec::with_capacity(files.len());
    for file in files {
        let started = std::time::Instant::now();
        let doc = match std::fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|t| RequestDocument::parse(&t).map_err(|e| e.to_string()))
        {
            Ok(d) => d,
            Err(msg) => {
                results.push(RunResult {
                    request_id: file.display().to_string(),
                    status: RunStatus::Error,
                    http: None,
                    assertions: Vec::new(),
                    runtime: BTreeMap::new(),
                    diagnostics: vec![Diagnostic::new(Code::InvalidAssetInput, msg)],
                    duration_ms: started.elapsed().as_millis() as u64,
                });
                continue;
            }
        };
        let result = run_with_runtime(
            &doc,
            root,
            file,
            env.clone(),
            secret,
            engine,
            mode,
            cancel.clone(),
            Value::Null,
            runtime.clone(),
        )
        .await;
        // Thread this request's runtime forward to the next.
        if let Value::Object(map) = &mut runtime {
            for (k, v) in &result.runtime {
                map.insert(k.clone(), v.clone());
            }
        }
        results.push(result);
    }
    results
}

/// Run one afterResponse/onError/finally asset. Builtins need a response;
/// without one they are skipped with an info note. JS assets always run,
/// receiving the response (if any) and error (onError/finally).
fn reaction_asset(
    entry: &super::ir::ResolvedPipelineEntry,
    ir: &ResolvedRequest,
    response: Option<&ResponseView>,
    error: Option<&str>,
) -> Result<(Vec<AssertionResult>, BTreeMap<String, Value>), Diagnostic> {
    if is_project_asset(entry) {
        run_js_after(entry, ir, response, error)
    } else {
        match response {
            Some(r) => run_after_response(entry, r),
            None => Err(Diagnostic::new(
                Code::AssetError,
                format!(
                    "builtin {:?} skipped: no response available in this phase",
                    entry.asset.raw
                ),
            )
            .info()
            .with_ref(&entry.asset.raw)),
        }
    }
}

/// Fold one reaction result into the accumulators (assertions, runtime with
/// conflict warnings, diagnostics, and the first error seen).
fn merge_reaction(
    outcome: Result<(Vec<AssertionResult>, BTreeMap<String, Value>), Diagnostic>,
    entry: &super::ir::ResolvedPipelineEntry,
    assertions: &mut Vec<AssertionResult>,
    runtime: &mut BTreeMap<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
    error: &mut Option<String>,
) {
    match outcome {
        Ok((results, extracted)) => {
            assertions.extend(results);
            for (k, v) in extracted {
                if runtime.insert(k.clone(), v).is_some() {
                    diagnostics.push(
                        Diagnostic::new(
                            Code::PipelineConflict,
                            format!("runtime key {k:?} written by more than one extractor"),
                        )
                        .with_ref(&entry.asset.raw),
                    );
                }
            }
        }
        Err(d) => {
            if d.is_error() {
                error.get_or_insert_with(|| d.message.clone());
            }
            diagnostics.push(d);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    ir: &ResolvedRequest,
    status: RunStatus,
    http: Option<HttpResultView>,
    assertions: Vec<AssertionResult>,
    runtime: BTreeMap<String, Value>,
    mut diagnostics: Vec<Diagnostic>,
    started: std::time::Instant,
) -> RunResult {
    // Mask secrets in every message that leaves the runner.
    for d in &mut diagnostics {
        d.message = ir.mask(&d.message);
    }
    RunResult {
        request_id: ir.id.clone(),
        status,
        http,
        assertions,
        runtime,
        diagnostics,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

// ---------------------------------------------------------------------
// Project (.js) assets via the QuickJS host (§15 trusted-local tier)
// ---------------------------------------------------------------------

/// JSON snapshot of the request handed to JS assets as `ctx.request`.
fn request_ctx(ir: &ResolvedRequest) -> Value {
    serde_json::json!({
        "method": ir.method.as_str(),
        "url": ir.url,
        "headers": ir.headers.iter().map(|h| serde_json::json!({"name": h.name, "value": h.value})).collect::<Vec<_>>(),
    })
}

fn run_js_hook(
    entry: &super::ir::ResolvedPipelineEntry,
    ir: &ResolvedRequest,
) -> Result<super::pipeline::RequestPatch, Diagnostic> {
    let ctx = serde_json::json!({ "request": request_ctx(ir), "bindings": ir.bindings });
    let out = super::jshost::run_js_asset(&entry.asset.address, &ctx, &entry.input)
        .map_err(|d| d.with_ref(&entry.asset.raw))?;

    let mut patch = super::pipeline::RequestPatch::default();
    if let Some(url) = out.get("url").and_then(Value::as_str) {
        patch.url = Some(url.to_string());
    }
    if let Some(headers) = out.get("headers").and_then(Value::as_array) {
        for h in headers {
            let (Some(name), Some(value)) =
                (h.get("name").and_then(Value::as_str), h.get("value").and_then(Value::as_str))
            else {
                return Err(Diagnostic::new(
                    Code::AssetError,
                    "hook returned a header without string name/value",
                )
                .with_ref(&entry.asset.raw));
            };
            patch.headers.push(ResolvedHeader { name: name.to_string(), value: value.to_string() });
        }
    }
    Ok(patch)
}

fn run_js_after(
    entry: &super::ir::ResolvedPipelineEntry,
    ir: &ResolvedRequest,
    response: Option<&ResponseView>,
    error: Option<&str>,
) -> Result<(Vec<AssertionResult>, BTreeMap<String, Value>), Diagnostic> {
    let response_ctx = response.map(|response| {
        serde_json::json!({
            "status": response.status,
            "headers": response.headers.iter().map(|(k, v)| serde_json::json!({"name": k, "value": v})).collect::<Vec<_>>(),
            "body": response.json().unwrap_or(Value::Null),
            "bodyText": response.text(),
            "timeMs": response.time_ms,
        })
    });
    let ctx = serde_json::json!({
        "request": request_ctx(ir),
        "bindings": ir.bindings,
        "response": response_ctx,
        "error": error,
    });
    let out = super::jshost::run_js_asset(&entry.asset.address, &ctx, &entry.input)
        .map_err(|d| d.with_ref(&entry.asset.raw))?;

    // The return shape decides the meaning (§5): `runtime` → extractor,
    // `passed` (object or array of objects) → assertion result(s).
    let mut assertions = Vec::new();
    let mut runtime = BTreeMap::new();
    if let Some(rt) = out.get("runtime").and_then(Value::as_object) {
        for (k, v) in rt {
            runtime.insert(k.clone(), v.clone());
        }
    }
    let mut push_assertion = |item: &Value| -> Result<(), Diagnostic> {
        let passed = item.get("passed").and_then(Value::as_bool).ok_or_else(|| {
            Diagnostic::new(
                Code::AssetError,
                "assertion result must have a boolean `passed`",
            )
            .with_ref(&entry.asset.raw)
        })?;
        assertions.push(AssertionResult {
            passed,
            message: item.get("message").and_then(Value::as_str).unwrap_or("(no message)").to_string(),
            expected: item.get("expected").cloned(),
            actual: item.get("actual").cloned(),
            path: item.get("path").and_then(Value::as_str).map(str::to_string),
        });
        Ok(())
    };
    match &out {
        Value::Array(items) => {
            for item in items {
                push_assertion(item)?;
            }
        }
        Value::Object(map) if map.contains_key("passed") => push_assertion(&out)?,
        Value::Object(map) if map.contains_key("runtime") => {}
        Value::Null => {}
        other => {
            return Err(Diagnostic::new(
                Code::AssetError,
                format!("unrecognized afterResponse asset result: {other}"),
            )
            .with_ref(&entry.asset.raw));
        }
    }
    Ok((assertions, runtime))
}

fn apply_patch(
    ir: &mut ResolvedRequest,
    headers: Vec<ResolvedHeader>,
    url: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(u) = url {
        ir.url = u;
    }
    for h in headers {
        // Upsert by case-insensitive name; warn on overwrite.
        if let Some(existing) = ir.headers.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&h.name)) {
            if existing.value != h.value {
                diagnostics.push(Diagnostic::new(
                    Code::PipelineConflict,
                    format!("header {} overwritten by a beforeRequest hook", h.name),
                ));
            }
            existing.value = h.value;
        } else {
            ir.headers.push(h);
        }
    }
}

/// Map the IR to the exec engine's request and send it.
async fn send(
    ir: &ResolvedRequest,
    engine: &HttpEngine,
    cancel: CancellationToken,
) -> Result<ResponseView, Diagnostic> {
    let mut exec = ExecRequest::new(ir.method, ir.url.clone());
    exec.headers = ir.headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect();
    for q in &ir.query {
        // Append query params to the URL.
        let sep = if exec.url.contains('?') { '&' } else { '?' };
        exec.url = format!(
            "{}{sep}{}={}",
            exec.url,
            urlencoding_encode(&q.name),
            urlencoding_encode(&q.value)
        );
    }
    exec.body = body_to_exec(&ir.body, &exec.headers);

    let result = engine
        .execute(exec, cancel)
        .await
        .map_err(|e| Diagnostic::new(Code::HttpError, format!("request failed: {e}")))?;

    Ok(ResponseView {
        status: result.status,
        headers: result.headers.clone(),
        body: result.body.clone(),
        time_ms: result.timing.total.as_millis() as u64,
    })
}

fn body_to_exec(body: &ResolvedBody, headers: &[(String, String)]) -> ExecBody {
    let has_ct = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    match body {
        ResolvedBody::None => ExecBody::None,
        ResolvedBody::Json(v) => {
            let data = serde_json::to_vec(v).unwrap_or_default();
            ExecBody::Bytes {
                content_type: (!has_ct).then(|| "application/json".to_string()),
                data,
            }
        }
        ResolvedBody::Text(s) => ExecBody::Bytes {
            content_type: (!has_ct).then(|| "text/plain".to_string()),
            data: s.clone().into_bytes(),
        },
        ResolvedBody::Form(fields) => {
            ExecBody::Form(fields.iter().map(|h| (h.name.clone(), h.value.clone())).collect())
        }
    }
}

/// Render a resolved request's mock to a [`ResponseView`] (static or the
/// dynamic JS mock). Used by the mock server. Returns the response and any
/// diagnostics; `None` if the document has no mock or the mock failed.
pub fn render_mock(ir: &ResolvedRequest) -> (Option<ResponseView>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let response = match &ir.mock {
        Some(m) => mock_response(m, ir, &mut diags),
        None => None,
    };
    (response, diags)
}

fn mock_response(
    mock: &ResolvedMock,
    ir: &ResolvedRequest,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<ResponseView> {
    match mock {
        ResolvedMock::Static { status, headers, body, .. } => {
            let body_bytes = match body {
                ResolvedBody::None => Vec::new(),
                ResolvedBody::Json(v) => serde_json::to_vec(v).unwrap_or_default(),
                ResolvedBody::Text(s) => s.clone().into_bytes(),
                ResolvedBody::Form(f) => f
                    .iter()
                    .map(|h| format!("{}={}", h.name, h.value))
                    .collect::<Vec<_>>()
                    .join("&")
                    .into_bytes(),
            };
            Some(ResponseView {
                status: *status,
                headers: headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect(),
                body: body_bytes,
                time_ms: 0,
            })
        }
        ResolvedMock::Dynamic { asset, input } => {
            let ctx = serde_json::json!({ "request": request_ctx(ir), "bindings": ir.bindings });
            let out = match super::jshost::run_js_asset(&asset.address, &ctx, input) {
                Ok(v) => v,
                Err(d) => {
                    diagnostics.push(d.with_ref(&asset.raw));
                    return None;
                }
            };
            let Some(status) = out.get("status").and_then(Value::as_u64) else {
                diagnostics.push(
                    Diagnostic::new(Code::AssetError, "mock asset must return { status, ... }")
                        .with_ref(&asset.raw),
                );
                return None;
            };
            let headers = out
                .get("headers")
                .and_then(Value::as_array)
                .map(|hs| {
                    hs.iter()
                        .filter_map(|h| {
                            Some((
                                h.get("name")?.as_str()?.to_string(),
                                h.get("value")?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let body = out
                .get("body")
                .map(|b| serde_json::to_vec(b).unwrap_or_default())
                .unwrap_or_default();
            Some(ResponseView { status: status as u16, headers, body, time_ms: 0 })
        }
    }
}

/// Minimal percent-encoding for query values (space and reserved chars).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
