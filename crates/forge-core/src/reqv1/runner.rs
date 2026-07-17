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
    let project = load_project(root).map_err(|d| vec![d])?;
    let resolver = RefResolver::new(root, &project).map_err(|e| e.0)?;
    let store = DataStore::new(&resolver);
    let base_dir = request_file.parent().unwrap_or(root);
    let inp = BuildInputs {
        resolver: &resolver,
        store: &store,
        base_dir,
        env,
        matrix: Value::Null,
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
) -> RunResult {
    let started = std::time::Instant::now();

    let mut ir = match validate(doc, root, request_file, env, secret) {
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

    // --- beforeRequest hooks ---
    for entry in ir.pipeline.clone().iter().filter(|e| e.phase == PipelinePhase::BeforeRequest) {
        if is_project_asset(entry) {
            diagnostics.push(project_asset_unsupported(&entry.asset.raw));
            continue;
        }
        match run_before_request(entry, &ir) {
            Ok(patch) => apply_patch(&mut ir, patch.headers, patch.url, &mut diagnostics),
            Err(d) => diagnostics.push(d),
        }
    }

    // --- send or mock ---
    let response = match mode {
        RunMode::Mock => match &ir.mock {
            Some(m) => mock_response(m, &mut diagnostics),
            None => {
                diagnostics.push(Diagnostic::new(
                    Code::HttpError,
                    "mock mode requested but the document has no mock",
                ));
                None
            }
        },
        RunMode::Http => match send(&ir, engine, cancel).await {
            Ok(r) => Some(r),
            Err(d) => {
                diagnostics.push(d);
                None
            }
        },
    };

    let Some(response) = response else {
        return finish(&ir, RunStatus::Error, None, Vec::new(), BTreeMap::new(), diagnostics, started);
    };

    // --- afterResponse assertions + extractors ---
    let mut assertions = Vec::new();
    let mut runtime: BTreeMap<String, Value> = BTreeMap::new();
    for entry in ir.pipeline.iter().filter(|e| e.phase == PipelinePhase::AfterResponse) {
        if is_project_asset(entry) {
            diagnostics.push(project_asset_unsupported(&entry.asset.raw));
            continue;
        }
        match run_after_response(entry, &response) {
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
            Err(d) => diagnostics.push(d),
        }
    }

    let http = Some(HttpResultView {
        status: response.status,
        time_ms: response.time_ms,
        bytes: response.body.len(),
    });
    let has_asset_error = diagnostics.iter().any(Diagnostic::is_error);
    let status = if has_asset_error {
        RunStatus::Error
    } else if assertions.iter().any(|a| !a.passed) {
        RunStatus::Failed
    } else {
        RunStatus::Passed
    };
    finish(&ir, status, http, assertions, runtime, diagnostics, started)
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

fn project_asset_unsupported(raw: &str) -> Diagnostic {
    Diagnostic::new(
        Code::AssetError,
        format!("project (TS/JS) asset {raw:?} not executable in v1 — use a builtin, or run on the JS host (extension point)"),
    )
    .with_ref(raw)
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

fn mock_response(mock: &ResolvedMock, diagnostics: &mut Vec<Diagnostic>) -> Option<ResponseView> {
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
        ResolvedMock::Dynamic { asset, .. } => {
            diagnostics.push(project_asset_unsupported(&asset.raw));
            None
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
