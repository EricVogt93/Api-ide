//! The run loop: sequential execution with variable chaining, scripts,
//! assertions and event streaming.

use std::collections::BTreeMap;
use std::path::Path;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::assert::{apply_extractors, evaluate_all};
use crate::exec::HttpEngine;
use crate::model::{AuthConfig, Environment, SecretValues};
use crate::script::ScriptEngine;
use crate::store::{CollectionNode, RequestNode, TreeNode, Workspace};
use crate::vars::VarScopes;

use super::{resolve_request, AuthChain, DataSource, RequestOutcome, RunError, RunEvent, RunOptions, RunScope, RunSummary};

/// A request node together with the ancestor context (collection/folder
/// variables and auth) needed to resolve it, precomputed once per run.
struct PlannedRequest<'a> {
    node: &'a RequestNode,
    collection_vars: &'a BTreeMap<String, String>,
    /// Nearest folder first.
    folder_vars: Vec<&'a BTreeMap<String, String>>,
    /// Nearest ancestor first, collection auth last (outermost).
    auth_chain: AuthChain<'a>,
}

/// Execute `scope` sequentially, streaming [`RunEvent`]s as they happen.
///
/// Extracted variables (extractors and script `vars.set`) feed the runtime
/// scope of subsequent requests. Returns the final summary (also emitted as
/// [`RunEvent::RunFinished`]). Respects `cancel` between and during requests.
pub async fn run(
    workspace: &Workspace,
    scope: RunScope,
    options: RunOptions,
    engine: &HttpEngine,
    events: UnboundedSender<RunEvent>,
    cancel: CancellationToken,
) -> Result<RunSummary, RunError> {
    let started = std::time::Instant::now();
    let planned = plan_requests(workspace, &scope)?;

    let env_pair: Option<(&Environment, &SecretValues)> = match &options.environment {
        Some(name) => {
            let loaded = workspace
                .environment(name)
                .ok_or_else(|| RunError::EnvironmentNotFound(name.clone()))?;
            Some((&loaded.env, &loaded.secrets))
        }
        None => None,
    };

    let iterations = load_iterations(&options.data)?;
    let iteration_count = iterations.len();
    let total = planned.len() * iteration_count;

    let _ = events.send(RunEvent::RunStarted { total, iterations: iteration_count });

    let script_engine = ScriptEngine::new();
    let mut runtime_vars: BTreeMap<String, String> = BTreeMap::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut stop_remaining = false;

    for (iter_idx, row) in iterations.iter().enumerate() {
        let _ = events.send(RunEvent::IterationStarted { iteration: iter_idx });

        for planned_req in &planned {
            let def = &planned_req.node.def;

            if stop_remaining || cancel.is_cancelled() {
                stop_remaining = true;
                skipped += 1;
                continue;
            }

            if def.settings.skip_in_runs {
                skipped += 1;
                continue;
            }

            let id = workspace.rel_id(&planned_req.node.file);
            let name = def.name.clone();

            let _ = events.send(RunEvent::RequestStarted {
                id: id.clone(),
                name: name.clone(),
                iteration: iter_idx,
            });

            let mut scopes = VarScopes::new()
                .with_collection(planned_req.collection_vars)
                .with_folders(planned_req.folder_vars.iter().copied());
            if let Some((env, secrets)) = env_pair {
                scopes = scopes.with_environment(env, secrets);
            }
            scopes.set_iteration_row(row.clone());
            for (k, v) in &runtime_vars {
                scopes.set_runtime(k.clone(), v.clone());
            }

            let outcome = execute_one(
                workspace,
                planned_req,
                &scopes,
                engine,
                &script_engine,
                &mut runtime_vars,
                id,
                name,
                iter_idx,
                &cancel,
            )
            .await;

            let ok = outcome.passed();
            let _ = events.send(RunEvent::RequestFinished(Box::new(outcome)));

            if ok {
                passed += 1;
            } else {
                failed += 1;
                if options.bail {
                    stop_remaining = true;
                }
            }

            if !stop_remaining && options.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(options.delay_ms)).await;
            }
        }
    }

    let summary = RunSummary {
        total,
        passed,
        failed,
        skipped,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    let _ = events.send(RunEvent::RunFinished(summary.clone()));
    Ok(summary)
}

/// Resolve, script and execute a single request, folding extractor / script
/// output into `runtime_vars` as it goes.
#[allow(clippy::too_many_arguments)]
async fn execute_one(
    workspace: &Workspace,
    planned: &PlannedRequest<'_>,
    scopes: &VarScopes,
    engine: &HttpEngine,
    script_engine: &ScriptEngine,
    runtime_vars: &mut BTreeMap<String, String>,
    id: String,
    name: String,
    iteration: usize,
    cancel: &CancellationToken,
) -> RequestOutcome {
    let def = &planned.node.def;

    let mut resolved = match resolve_request(workspace, def, &planned.auth_chain, scopes, engine).await {
        Ok(r) => r,
        Err(e) => {
            return RequestOutcome {
                id,
                name,
                iteration,
                result: Err(e.to_string()),
                assertions: Vec::new(),
                script_log: Vec::new(),
                script_error: None,
                extracted: Vec::new(),
            };
        }
    };

    let mut script_log = Vec::new();
    let mut script_error = None;

    if let Some(pre) = &def.scripts.pre_request {
        let out = script_engine.run_pre(pre, &mut resolved, runtime_vars);
        script_log.extend(out.log);
        for (k, v) in out.vars_set {
            runtime_vars.insert(k, v);
        }
        script_error = out.error;
    }

    if let Some(err) = script_error {
        return RequestOutcome {
            id,
            name,
            iteration,
            result: Err("pre-request script failed".to_string()),
            assertions: Vec::new(),
            script_log,
            script_error: Some(err),
            extracted: Vec::new(),
        };
    }

    let exec_result = match engine.execute(resolved, cancel.clone()).await {
        Ok(r) => r,
        Err(e) => {
            return RequestOutcome {
                id,
                name,
                iteration,
                result: Err(e.to_string()),
                assertions: Vec::new(),
                script_log,
                script_error: None,
                extracted: Vec::new(),
            };
        }
    };

    let mut assertions = evaluate_all(&def.assertions, &exec_result);

    let extract_report = apply_extractors(&def.extractors, &exec_result);
    let extracted = extract_report.values.clone();
    for (k, v) in &extract_report.values {
        runtime_vars.insert(k.clone(), v.clone());
    }
    for err in &extract_report.errors {
        script_log.push(format!("extract: {err}"));
    }

    let mut script_error = None;
    if let Some(post) = &def.scripts.post_response {
        let out = script_engine.run_post(post, &exec_result, runtime_vars);
        script_log.extend(out.log);
        assertions.extend(out.assertions);
        for (k, v) in out.vars_set {
            runtime_vars.insert(k, v);
        }
        script_error = out.error;
    }

    RequestOutcome {
        id,
        name,
        iteration,
        result: Ok(exec_result),
        assertions,
        script_log,
        script_error,
        extracted,
    }
}

/// Trim any leading/trailing `/` for comparing workspace-relative directory
/// paths regardless of how the caller formatted `scope`.
fn norm(s: &str) -> &str {
    s.trim_matches('/')
}

fn plan_requests<'a>(workspace: &'a Workspace, scope: &RunScope) -> Result<Vec<PlannedRequest<'a>>, RunError> {
    match scope {
        RunScope::Request(id) => {
            let node = workspace.find_request(id).ok_or_else(|| RunError::ScopeNotFound(id.clone()))?;
            let planned = find_request_planned(workspace, &node.file)
                .ok_or_else(|| RunError::ScopeNotFound(id.clone()))?;
            Ok(vec![planned])
        }
        RunScope::Collection(rel) => {
            let mut out = Vec::new();
            let mut found = false;
            for col in &workspace.collections {
                if norm(&workspace.rel_id(&col.dir)) == norm(rel) {
                    found = true;
                    collect_subtree(col, &col.children, Vec::new(), &mut out);
                }
            }
            if !found {
                return Err(RunError::ScopeNotFound(rel.clone()));
            }
            Ok(out)
        }
        RunScope::Folder(rel) => {
            let mut out = Vec::new();
            let mut found = false;
            for col in &workspace.collections {
                if collect_folder(workspace, col, &col.children, Vec::new(), rel, &mut out) {
                    found = true;
                }
            }
            if !found {
                return Err(RunError::ScopeNotFound(rel.clone()));
            }
            Ok(out)
        }
        RunScope::Workspace => {
            let mut out = Vec::new();
            for col in &workspace.collections {
                collect_subtree(col, &col.children, Vec::new(), &mut out);
            }
            Ok(out)
        }
    }
}

type Ancestors<'a> = Vec<(&'a BTreeMap<String, String>, &'a AuthConfig)>;

fn build_planned<'a>(
    col: &'a CollectionNode,
    ancestors: &Ancestors<'a>,
    node: &'a RequestNode,
) -> PlannedRequest<'a> {
    let folder_vars: Vec<&'a BTreeMap<String, String>> = ancestors.iter().rev().map(|(v, _)| *v).collect();
    let mut auth_chain: AuthChain<'a> = ancestors.iter().rev().map(|(_, a)| *a).collect();
    auth_chain.push(&col.meta.auth);
    PlannedRequest { node, collection_vars: &col.meta.variables, folder_vars, auth_chain }
}

/// Collect every request under `children` unconditionally (depth-first,
/// tree order preserved).
fn collect_subtree<'a>(
    col: &'a CollectionNode,
    children: &'a [TreeNode],
    ancestors: Ancestors<'a>,
    out: &mut Vec<PlannedRequest<'a>>,
) {
    for child in children {
        match child {
            TreeNode::Request(r) => out.push(build_planned(col, &ancestors, r)),
            TreeNode::Folder(f) => {
                let mut next = ancestors.clone();
                next.push((&f.meta.variables, &f.meta.auth));
                collect_subtree(col, &f.children, next, out);
            }
        }
    }
}

/// Search `children` for a folder whose workspace-relative directory
/// matches `rel`; if found, collect its whole subtree into `out` and return
/// `true`.
fn collect_folder<'a>(
    workspace: &'a Workspace,
    col: &'a CollectionNode,
    children: &'a [TreeNode],
    ancestors: Ancestors<'a>,
    rel: &str,
    out: &mut Vec<PlannedRequest<'a>>,
) -> bool {
    for child in children {
        if let TreeNode::Folder(f) = child {
            let mut next = ancestors.clone();
            next.push((&f.meta.variables, &f.meta.auth));
            if norm(&workspace.rel_id(&f.dir)) == norm(rel) {
                collect_subtree(col, &f.children, next, out);
                return true;
            }
            if collect_folder(workspace, col, &f.children, next, rel, out) {
                return true;
            }
        }
    }
    false
}

/// Find a single request by absolute file path and build its ancestor
/// context.
fn find_request_planned<'a>(workspace: &'a Workspace, target: &Path) -> Option<PlannedRequest<'a>> {
    for col in &workspace.collections {
        if let Some(p) = find_request_in(col, &col.children, Vec::new(), target) {
            return Some(p);
        }
    }
    None
}

fn find_request_in<'a>(
    col: &'a CollectionNode,
    children: &'a [TreeNode],
    ancestors: Ancestors<'a>,
    target: &Path,
) -> Option<PlannedRequest<'a>> {
    for child in children {
        match child {
            TreeNode::Request(r) if r.file == target => return Some(build_planned(col, &ancestors, r)),
            TreeNode::Request(_) => {}
            TreeNode::Folder(f) => {
                let mut next = ancestors.clone();
                next.push((&f.meta.variables, &f.meta.auth));
                if let Some(p) = find_request_in(col, &f.children, next, target) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Build the list of data-driven iteration rows. `None` means a single,
/// empty-row iteration.
fn load_iterations(data: &Option<DataSource>) -> Result<Vec<BTreeMap<String, String>>, RunError> {
    match data {
        None => Ok(vec![BTreeMap::new()]),
        Some(DataSource::CsvFile(path)) => {
            let mut reader = csv::Reader::from_path(path)
                .map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?;
            let headers = reader
                .headers()
                .map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?
                .clone();
            let mut rows = Vec::new();
            for record in reader.records() {
                let record = record.map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?;
                let mut row = BTreeMap::new();
                for (h, v) in headers.iter().zip(record.iter()) {
                    row.insert(h.to_string(), v.to_string());
                }
                rows.push(row);
            }
            Ok(rows)
        }
        Some(DataSource::JsonFile(path)) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?;
            let arr = value
                .as_array()
                .ok_or_else(|| RunError::Data(format!("{}: expected a JSON array of objects", path.display())))?;
            let mut rows = Vec::new();
            for item in arr {
                let obj = item.as_object().ok_or_else(|| {
                    RunError::Data(format!("{}: expected array items to be objects", path.display()))
                })?;
                let mut row = BTreeMap::new();
                for (k, v) in obj {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    row.insert(k.clone(), s);
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
}
