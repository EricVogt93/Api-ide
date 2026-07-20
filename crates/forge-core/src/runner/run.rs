//! The run loop: sequential execution with variable chaining, scripts,
//! assertions and event streaming.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::assert::{apply_extractors, evaluate_all};
use crate::exec::HttpEngine;
use crate::model::{AuthConfig, Environment, SecretValues};
use crate::script::{ScriptOutput, Scripting, VarMutation};
use crate::store::{CollectionNode, RequestNode, TreeNode, Workspace};
use crate::vars::VarScopes;

use super::{
    resolve_request, AuthChain, DataSource, RequestOutcome, RunError, RunEvent, RunOptions,
    RunScope, RunSummary,
};

/// A request node together with the ancestor context (collection/folder
/// variables and auth) needed to resolve it, precomputed once per run.
struct PlannedRequest<'a> {
    node: &'a RequestNode,
    collection_vars: &'a BTreeMap<String, String>,
    /// Nearest folder first.
    folder_vars: Vec<&'a BTreeMap<String, String>>,
    /// Nearest ancestor first, collection auth last (outermost).
    auth_chain: AuthChain<'a>,
    /// Suite lifecycle hooks in scope for this request, outermost
    /// (collection) first, innermost (nearest folder) last.
    hook_chain: Vec<HookScope<'a>>,
}

/// One collection/folder's suite hooks, identified by its directory (used
/// to fire `beforeAll`/`afterAll` exactly once per run per scope).
#[derive(Clone, Copy)]
struct HookScope<'a> {
    dir: &'a Path,
    hooks: &'a crate::model::SuiteHooks,
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

    let _ = events.send(RunEvent::RunStarted {
        total,
        iterations: iteration_count,
    });

    // `beforeAll`/`afterAll` fire once per run (not once per data-driven
    // iteration): precompute, for every collection/folder that has at least
    // one non-`skip_in_runs` request under it, the static planned-list
    // index of its first and last such request.
    let (first_index, last_index) = hook_scope_bounds(&planned);

    let scripting = Scripting::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut stop_remaining = false;

    for (iter_idx, row) in iterations.iter().enumerate() {
        // Fresh per iteration: chaining (extractors/scripts feeding later
        // requests) is an intra-iteration concept, so a value extracted in
        // row 1 must not leak into row 2.
        let mut runtime_vars: BTreeMap<String, String> = BTreeMap::new();

        if !stop_remaining && !cancel.is_cancelled() {
            let _ = events.send(RunEvent::IterationStarted {
                iteration: iter_idx,
            });
        }

        for (planned_idx, planned_req) in planned.iter().enumerate() {
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

            // Hooks run first — they mutate `runtime_vars`, and the var
            // scope built below must see whatever they set (e.g. a
            // `beforeEach` value used in this very request's URL/headers).
            let mut outcome = match run_before_hooks(
                &scripting,
                planned_req,
                planned_idx,
                iter_idx,
                &first_index,
                &mut runtime_vars,
            ) {
                Err(failure) => RequestOutcome {
                    id: id.clone(),
                    name: name.clone(),
                    iteration: iter_idx,
                    result: Err(failure.message),
                    assertions: Vec::new(),
                    script_log: failure.log,
                    script_error: None,
                    extracted: Vec::new(),
                },
                Ok(before_log) => {
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

                    let mut outcome = execute_one(
                        workspace,
                        planned_req,
                        &scopes,
                        engine,
                        &scripting,
                        &mut runtime_vars,
                        id,
                        name,
                        iter_idx,
                        &cancel,
                    )
                    .await;
                    if !before_log.is_empty() {
                        let mut log = before_log;
                        log.append(&mut outcome.script_log);
                        outcome.script_log = log;
                    }
                    outcome
                }
            };

            run_after_hooks(
                &scripting,
                planned_req,
                planned_idx,
                iter_idx,
                iteration_count,
                &last_index,
                &mut runtime_vars,
                &mut outcome,
            );

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

/// For every collection/folder dir that has at least one non-`skip_in_runs`
/// request under it, the static planned-list index of its first
/// (`first_index`) and last (`last_index`) such request.
fn hook_scope_bounds(
    planned: &[PlannedRequest<'_>],
) -> (HashMap<PathBuf, usize>, HashMap<PathBuf, usize>) {
    let mut first_index = HashMap::new();
    let mut last_index = HashMap::new();
    for (idx, p) in planned.iter().enumerate() {
        if p.node.def.settings.skip_in_runs {
            continue;
        }
        for scope in &p.hook_chain {
            first_index.entry(scope.dir.to_path_buf()).or_insert(idx);
            last_index.insert(scope.dir.to_path_buf(), idx);
        }
    }
    (first_index, last_index)
}

/// A `beforeAll`/`beforeEach` hook failure: the request it was guarding
/// never runs.
struct HookFailure {
    message: String,
    log: Vec<String>,
}

fn apply_var_mutations(runtime_vars: &mut BTreeMap<String, String>, output: &ScriptOutput) {
    for mutation in &output.var_mutations {
        match mutation {
            VarMutation::Set(name, value) => {
                runtime_vars.insert(name.clone(), value.clone());
            }
            VarMutation::Unset(name) => {
                runtime_vars.remove(name);
            }
        }
    }
}

/// Run every applicable `beforeAll` (only for scopes whose first executed
/// request is this one, and only in the first iteration) then `beforeEach`
/// hook for `planned_req`, outermost scope first. Vars set by hooks that do
/// run — including the one that fails — are merged into `runtime_vars`
/// immediately. `Ok(log)` carries every hook's log lines (prefixed `hook:`)
/// to be prepended to the request's own outcome; `Err` means the first
/// hook error, and the request never executes.
fn run_before_hooks(
    scripting: &Scripting,
    planned_req: &PlannedRequest<'_>,
    planned_idx: usize,
    iter_idx: usize,
    first_index: &HashMap<PathBuf, usize>,
    runtime_vars: &mut BTreeMap<String, String>,
) -> Result<Vec<String>, HookFailure> {
    let mut log = Vec::new();
    for scope in &planned_req.hook_chain {
        if iter_idx == 0 && first_index.get(scope.dir) == Some(&planned_idx) {
            if let Some(script) = &scope.hooks.before_all {
                let out = scripting.run_hook(scope.hooks.language, script, runtime_vars);
                apply_var_mutations(runtime_vars, &out);
                log.extend(out.log.into_iter().map(|l| format!("hook: {l}")));
                if let Some(err) = out.error {
                    return Err(HookFailure {
                        message: format!("beforeAll hook failed: {err}"),
                        log,
                    });
                }
            }
        }
        if let Some(script) = &scope.hooks.before_each {
            let out = scripting.run_hook(scope.hooks.language, script, runtime_vars);
            apply_var_mutations(runtime_vars, &out);
            log.extend(out.log.into_iter().map(|l| format!("hook: {l}")));
            if let Some(err) = out.error {
                return Err(HookFailure {
                    message: format!("beforeEach hook failed: {err}"),
                    log,
                });
            }
        }
    }
    Ok(log)
}

/// Run every applicable `afterEach` then `afterAll` (only for scopes whose
/// last executed request is this one, and only in the final iteration) hook
/// for `planned_req`, innermost scope first (the reverse of
/// [`run_before_hooks`]). Both get `res` when the request actually
/// produced one; hook errors never flip `outcome.passed()` — they're
/// appended to `outcome.script_log` instead.
#[allow(clippy::too_many_arguments)]
fn run_after_hooks(
    scripting: &Scripting,
    planned_req: &PlannedRequest<'_>,
    planned_idx: usize,
    iter_idx: usize,
    iteration_count: usize,
    last_index: &HashMap<PathBuf, usize>,
    runtime_vars: &mut BTreeMap<String, String>,
    outcome: &mut RequestOutcome,
) {
    let Ok(exec_result) = outcome.result.clone() else {
        return;
    };
    let is_final_iter = iter_idx + 1 == iteration_count;

    for scope in planned_req.hook_chain.iter().rev() {
        if let Some(script) = &scope.hooks.after_each {
            apply_post_hook(
                scripting,
                scope.hooks.language,
                script,
                &exec_result,
                runtime_vars,
                outcome,
                "afterEach",
            );
        }
        if is_final_iter && last_index.get(scope.dir) == Some(&planned_idx) {
            if let Some(script) = &scope.hooks.after_all {
                apply_post_hook(
                    scripting,
                    scope.hooks.language,
                    script,
                    &exec_result,
                    runtime_vars,
                    outcome,
                    "afterAll",
                );
            }
        }
    }
}

/// Run one `afterEach`/`afterAll` script and fold its output into
/// `outcome`: log lines (prefixed `hook:`), assertions, and `vars.set`
/// calls always merge in; an error is appended to the log (prefixed with
/// `label`) without failing the request.
#[allow(clippy::too_many_arguments)]
fn apply_post_hook(
    scripting: &Scripting,
    lang: crate::model::ScriptLang,
    script: &str,
    res: &crate::exec::ExecutionResult,
    runtime_vars: &mut BTreeMap<String, String>,
    outcome: &mut RequestOutcome,
    label: &str,
) {
    let out = scripting.run_post(lang, script, res, runtime_vars);
    apply_var_mutations(runtime_vars, &out);
    outcome
        .script_log
        .extend(out.log.into_iter().map(|l| format!("hook: {l}")));
    outcome.assertions.extend(out.assertions);
    if let Some(err) = out.error {
        outcome.script_log.push(format!("{label}: {err}"));
    }
}

/// Resolve, script and execute a single request, folding extractor / script
/// output into `runtime_vars` as it goes.
#[allow(clippy::too_many_arguments)]
async fn execute_one(
    workspace: &Workspace,
    planned: &PlannedRequest<'_>,
    scopes: &VarScopes,
    engine: &HttpEngine,
    scripting: &Scripting,
    runtime_vars: &mut BTreeMap<String, String>,
    id: String,
    name: String,
    iteration: usize,
    cancel: &CancellationToken,
) -> RequestOutcome {
    let def = &planned.node.def;

    let mut resolved =
        match resolve_request(workspace, def, &planned.auth_chain, scopes, engine).await {
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
    let mut pre_assertions = Vec::new();

    if let Some(pre) = &def.scripts.pre_request {
        let out = scripting.run_pre(def.scripts.language, pre, &mut resolved, runtime_vars);
        apply_var_mutations(runtime_vars, &out);
        script_log.extend(out.log);
        // `pm.test` may run in pre-request scripts; keep those outcomes.
        pre_assertions = out.assertions;
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

    let mut assertions = pre_assertions;
    assertions.extend(evaluate_all(
        &super::resolve::resolve_assertions(&def.assertions, scopes),
        &exec_result,
    ));

    let extract_report = apply_extractors(&def.extractors, &exec_result);
    let extracted = extract_report.values.clone();
    for (k, v) in &extract_report.values {
        runtime_vars.insert(k.clone(), v.clone());
    }
    for err in &extract_report.errors {
        script_log.push(format!("extract: {err}"));
    }
    // Drop any stale value left over from a previous iteration for every
    // enabled extractor that failed this time, so a failed extraction never
    // silently reuses an older (possibly no-longer-valid) value.
    let succeeded: std::collections::HashSet<&str> = extract_report
        .values
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    for ext in &def.extractors {
        if ext.enabled && !succeeded.contains(ext.var.as_str()) {
            runtime_vars.remove(&ext.var);
        }
    }

    let mut script_error = None;
    if let Some(post) = &def.scripts.post_response {
        let out = scripting.run_post(def.scripts.language, post, &exec_result, runtime_vars);
        apply_var_mutations(runtime_vars, &out);
        script_log.extend(out.log);
        assertions.extend(out.assertions);
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

fn plan_requests<'a>(
    workspace: &'a Workspace,
    scope: &RunScope,
) -> Result<Vec<PlannedRequest<'a>>, RunError> {
    match scope {
        RunScope::Request(id) => {
            let node = workspace
                .find_request(id)
                .ok_or_else(|| RunError::ScopeNotFound(id.clone()))?;
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

/// Nearest-ancestor-last: outermost folder first, immediate parent folder
/// last (collection is implicit — not included here, added separately).
type Ancestors<'a> = Vec<(
    &'a Path,
    &'a BTreeMap<String, String>,
    &'a AuthConfig,
    &'a crate::model::SuiteHooks,
)>;

fn build_planned<'a>(
    col: &'a CollectionNode,
    ancestors: &Ancestors<'a>,
    node: &'a RequestNode,
) -> PlannedRequest<'a> {
    let folder_vars: Vec<&'a BTreeMap<String, String>> =
        ancestors.iter().rev().map(|(_, v, _, _)| *v).collect();
    let mut auth_chain: AuthChain<'a> = ancestors.iter().rev().map(|(_, _, a, _)| *a).collect();
    auth_chain.push(&col.meta.auth);
    let mut hook_chain = vec![HookScope {
        dir: &col.dir,
        hooks: &col.meta.hooks,
    }];
    hook_chain.extend(
        ancestors
            .iter()
            .map(|(dir, _, _, hooks)| HookScope { dir, hooks }),
    );
    PlannedRequest {
        node,
        collection_vars: &col.meta.variables,
        folder_vars,
        auth_chain,
        hook_chain,
    }
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
                next.push((&f.dir, &f.meta.variables, &f.meta.auth, &f.meta.hooks));
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
            next.push((&f.dir, &f.meta.variables, &f.meta.auth, &f.meta.hooks));
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
            TreeNode::Request(r) if r.file == target => {
                return Some(build_planned(col, &ancestors, r))
            }
            TreeNode::Request(_) => {}
            TreeNode::Folder(f) => {
                let mut next = ancestors.clone();
                next.push((&f.dir, &f.meta.variables, &f.meta.auth, &f.meta.hooks));
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
                let record =
                    record.map_err(|e| RunError::Data(format!("{}: {e}", path.display())))?;
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
            let arr = value.as_array().ok_or_else(|| {
                RunError::Data(format!(
                    "{}: expected a JSON array of objects",
                    path.display()
                ))
            })?;
            let mut rows = Vec::new();
            for item in arr {
                let obj = item.as_object().ok_or_else(|| {
                    RunError::Data(format!(
                        "{}: expected array items to be objects",
                        path.display()
                    ))
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
