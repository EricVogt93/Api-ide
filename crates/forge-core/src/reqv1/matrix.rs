//! Matrix parameterization (§13): each `matrix` binding must resolve to an
//! array; the request runs once per element (cartesian product across
//! multiple names). Runtime is per-iteration; bindings re-resolve per case.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::exec::HttpEngine;

use super::build::BuildInputs;
use super::diag::{Code, Diagnostic, Errors};
use super::model::{Binding, RequestDocument};
use super::refs::RefResolver;
use super::resolve::DataStore;
use super::runner::{load_project, run_with_response_in_session, AuthSession, RunMode, RunResult};

/// One matrix case: name → element, e.g. `{ "case": {…} }`, referenced in the
/// document as `${matrix.case.*}`.
pub type MatrixCase = Map<String, Value>;

/// Resolve the document's `matrix` block into the list of cases (§13).
/// Empty matrix → one implicit empty case (a single plain run).
pub fn resolve_cases(
    matrix: &BTreeMap<String, Binding>,
    resolver: &RefResolver,
    store: &DataStore<'_>,
    base_dir: &Path,
    env: &Value,
    secret: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<Vec<MatrixCase>, Errors> {
    if matrix.is_empty() {
        return Ok(vec![Map::new()]);
    }

    // Resolve each matrix binding to an array. Matrix bindings may use
    // env/secret but not `${bindings.*}` (matrix is the outer loop, §7).
    let mut arrays: Vec<(String, Vec<Value>)> = Vec::new();
    let mut errors = Vec::new();
    for (name, binding) in matrix {
        let empty = Value::Object(Map::new());
        let inp = BuildInputs {
            resolver,
            store,
            base_dir,
            env: env.clone(),
            matrix: empty,
            runtime: Value::Object(Default::default()),
            secret,
        };
        match super::build::resolve_single_binding(binding, &inp) {
            Ok(Value::Array(items)) => arrays.push((name.clone(), items)),
            Ok(other) => errors.push(
                Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!(
                        "matrix binding {name:?} must resolve to an array, got {}",
                        type_name(&other)
                    ),
                )
                .at(format!("/matrix/{name}")),
            ),
            Err(mut d) => {
                d.instance_path = Some(format!("/matrix/{name}"));
                errors.push(d);
            }
        }
    }
    if !errors.is_empty() {
        return Err(Errors(errors));
    }

    // Cartesian product in declaration order.
    let mut cases: Vec<MatrixCase> = vec![Map::new()];
    for (name, items) in arrays {
        let mut next = Vec::with_capacity(cases.len() * items.len());
        for case in &cases {
            for item in &items {
                let mut c = case.clone();
                c.insert(name.clone(), item.clone());
                next.push(c);
            }
        }
        cases = next;
    }
    Ok(cases)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Run a document across all its matrix cases. A document without a matrix
/// yields exactly one result. Each result carries its case values (masked
/// display is the caller's concern; secrets never enter matrix values by
/// §7 — matrix resolves before secrets are interpolated into requests).
#[allow(clippy::too_many_arguments)]
pub async fn run_matrix(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &(dyn Fn(&str) -> Option<String> + Sync),
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
) -> Result<Vec<(MatrixCase, RunResult)>, Errors> {
    run_matrix_with_responses(doc, root, request_file, env, secret, engine, mode, cancel)
        .await
        .map(|results| {
            results
                .into_iter()
                .map(|(case, result, _)| (case, result))
                .collect()
        })
}

/// [`run_matrix`] while retaining each case's response for interactive GUI
/// inspection. CLI and headless callers should keep using [`run_matrix`].
#[allow(clippy::too_many_arguments)]
pub async fn run_matrix_with_responses(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &(dyn Fn(&str) -> Option<String> + Sync),
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
) -> Result<Vec<(MatrixCase, RunResult, Option<super::pipeline::ResponseView>)>, Errors> {
    let auth = AuthSession::default();
    run_matrix_with_responses_in_session(
        doc,
        root,
        request_file,
        env,
        secret,
        engine,
        mode,
        cancel,
        &auth,
    )
    .await
}

/// [`run_matrix_with_responses`] using a caller-owned auth cache.
#[allow(clippy::too_many_arguments)]
pub async fn run_matrix_with_responses_in_session(
    doc: &RequestDocument,
    root: &Path,
    request_file: &Path,
    env: Value,
    secret: &(dyn Fn(&str) -> Option<String> + Sync),
    engine: &HttpEngine,
    mode: RunMode,
    cancel: CancellationToken,
    auth: &AuthSession,
) -> Result<Vec<(MatrixCase, RunResult, Option<super::pipeline::ResponseView>)>, Errors> {
    let project = load_project(root).map_err(|d| Errors(vec![d]))?;
    let resolver = RefResolver::new(root, &project)?;
    let store = DataStore::new(&resolver);
    let base_dir = request_file.parent().unwrap_or(root);
    let cases = resolve_cases(&doc.matrix, &resolver, &store, base_dir, &env, secret)?;

    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let (result, response) = run_with_response_in_session(
            doc,
            root,
            request_file,
            env.clone(),
            secret,
            engine,
            mode,
            cancel.clone(),
            Value::Object(case.clone()),
            auth,
        )
        .await;
        results.push((case, result, response));
    }
    Ok(results)
}
