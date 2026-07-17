//! Document → canonical IR: resolve bindings (with cycle detection), run
//! built-in generators, interpolate variables, and assemble a
//! [`ResolvedRequest`]. Pure (no network). See §6–§8, §12.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};

use super::diag::{Code, Diagnostic, Errors};
use super::ir::{ResolvedBody, ResolvedHeader, ResolvedMock, ResolvedPipelineEntry, ResolvedRequest};
use super::model::{
    Binding, BodySpec, MockDef, PipelineEntry, RequestDocument, RequestSpec,
};
use super::refs::{RefResolver, RefScheme};
use super::resolve::DataStore;
use super::vars::{interpolate, Scopes, SecretSink};

/// Everything the builder needs beyond the document itself.
pub struct BuildInputs<'a> {
    pub resolver: &'a RefResolver,
    pub store: &'a DataStore<'a>,
    /// Directory of the request file (for relative refs).
    pub base_dir: &'a Path,
    pub env: Value,
    /// One matrix case (object), or Null when not a matrix run.
    pub matrix: Value,
    pub secret: &'a dyn Fn(&str) -> Option<String>,
}

/// Build the canonical IR from a parsed document. Collects every independent
/// error before failing (§7).
pub fn build_ir(doc: &RequestDocument, inp: &BuildInputs<'_>) -> Result<ResolvedRequest, Errors> {
    let mut sink = SecretSink::default();
    let mut errors: Vec<Diagnostic> = Vec::new();

    // 1. Resolve bindings (topological, cycle-checked, generators run here).
    let bindings = match resolve_bindings(&doc.bindings, inp, &mut sink) {
        Ok(v) => v,
        Err(mut e) => {
            errors.append(&mut e.0);
            Value::Object(Map::new())
        }
    };

    let empty = Value::Object(Map::new());
    let scopes = Scopes {
        env: &inp.env,
        bindings: &bindings,
        matrix: &inp.matrix,
        runtime: &empty,
        secret: inp.secret,
    };

    // 2. Interpolate the request itself.
    let request = build_request(&doc.request, inp, &scopes, &mut sink, &mut errors);

    // 3. Resolve the pipeline (locate assets, interpolate `with`).
    let pipeline = build_pipeline(&doc.pipeline, inp, &scopes, &mut sink, &mut errors);

    // 4. Resolve the mock (if any).
    let mock = doc
        .mock
        .as_ref()
        .and_then(|m| build_mock(m, inp, &scopes, &mut sink, &mut errors));

    if !errors.is_empty() {
        return Err(Errors(errors));
    }
    let request = request.expect("no errors implies a request");

    Ok(ResolvedRequest {
        id: doc.meta.id.clone(),
        name: doc.meta.name.clone(),
        method: request.method,
        url: request.url,
        headers: request.headers,
        query: request.query,
        body: request.body,
        pipeline,
        mock,
        bindings,
        secret_values: sink.values,
    })
}

struct ResolvedReqParts {
    method: crate::model::Method,
    url: String,
    headers: Vec<ResolvedHeader>,
    query: Vec<ResolvedHeader>,
    body: ResolvedBody,
}

// ---------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------

/// Resolve all bindings into a JSON object. Bindings may reference each other
/// via `${bindings.x}`; resolution order is a topological sort of that
/// dependency graph, and a cycle is a `BINDING_CYCLE` error.
fn resolve_bindings(
    bindings: &BTreeMap<String, Binding>,
    inp: &BuildInputs<'_>,
    sink: &mut SecretSink,
) -> Result<Value, Errors> {
    // Dependency graph over `${bindings.NAME}` references.
    let mut deps: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (name, binding) in bindings {
        deps.insert(name, binding_deps(binding));
    }

    let order = topo_order(&deps).map_err(|cycle| {
        Errors::one(
            Code::BindingCycle,
            format!("binding cycle: {}", cycle.join(" -> ")),
        )
    })?;

    let mut resolved = Map::new();
    let mut errors = Vec::new();
    for name in order {
        let binding = &bindings[name];
        let partial = Value::Object(resolved.clone());
        match resolve_one_binding(binding, inp, &partial, sink) {
            Ok(v) => {
                resolved.insert(name.to_string(), v);
            }
            Err(mut d) => {
                d.instance_path = Some(format!("/bindings/{name}"));
                errors.push(d);
            }
        }
    }
    if !errors.is_empty() {
        return Err(Errors(errors));
    }
    Ok(Value::Object(resolved))
}

/// Resolve one binding in isolation (no other bindings in scope). Used by
/// matrix resolution, where `${bindings.*}` is not available (§13).
pub fn resolve_single_binding(
    binding: &Binding,
    inp: &BuildInputs<'_>,
) -> Result<Value, Diagnostic> {
    let empty = Value::Object(Map::new());
    let mut sink = SecretSink::default();
    resolve_one_binding(binding, inp, &empty, &mut sink)
}

fn resolve_one_binding(
    binding: &Binding,
    inp: &BuildInputs<'_>,
    resolved_bindings: &Value,
    sink: &mut SecretSink,
) -> Result<Value, Diagnostic> {
    let empty = Value::Object(Map::new());
    let scopes = Scopes {
        env: &inp.env,
        bindings: resolved_bindings,
        matrix: &inp.matrix,
        runtime: &empty,
        secret: inp.secret,
    };
    match binding {
        Binding::Value(v) => interpolate(&v.value, &scopes, sink),
        Binding::Ref(r) => {
            let desc = inp.resolver.resolve(&r.reference, inp.base_dir)?;
            let value = inp.store.resolve(&desc, &r.patch)?;
            // §6 step 8: data content is NOT re-scanned for ${...}.
            Ok(value)
        }
        Binding::Use(u) => {
            let desc = inp.resolver.resolve(&u.uses, inp.base_dir)?;
            if desc.scheme != RefScheme::Builtin {
                // Project generators run on the JS host — not wired in v1.
                return Err(Diagnostic::new(
                    Code::AssetError,
                    format!(
                        "project generator {:?} not supported yet (v1 supports builtin generators)",
                        u.uses
                    ),
                )
                .with_ref(&u.uses));
            }
            let input = interpolate(&Value::Object(u.with.clone()), &scopes, sink)?;
            run_generator(&desc.address, desc.version, &input, &u.uses)
        }
    }
}

/// Names of other bindings a binding depends on (via `${bindings.NAME}`).
fn binding_deps(binding: &Binding) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match binding {
        Binding::Value(v) => collect_binding_refs(&v.value, &mut out),
        Binding::Use(u) => collect_binding_refs(&Value::Object(u.with.clone()), &mut out),
        Binding::Ref(_) => {}
    }
    out
}

/// Scan a JSON value for `${bindings.NAME...}` and collect each NAME.
fn collect_binding_refs(node: &Value, out: &mut BTreeSet<String>) {
    match node {
        Value::String(s) => {
            let bytes = s.as_bytes();
            let mut i = 0;
            while let Some(rel) = s[i..].find("${bindings.") {
                let start = i + rel + "${bindings.".len();
                let rest = &s[start..];
                let end = rest.find(['.', '}']).unwrap_or(rest.len());
                if !rest[..end].is_empty() {
                    out.insert(rest[..end].to_string());
                }
                i = start;
                if i >= bytes.len() {
                    break;
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_binding_refs(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_binding_refs(v, out)),
        _ => {}
    }
}

/// Kahn topological sort. Returns node order, or a cycle path on failure.
/// Edges only to nodes that exist in `deps` (external refs are ignored here;
/// missing bindings surface as MISSING_VARIABLE during interpolation).
fn topo_order<'a>(deps: &'a BTreeMap<&'a str, BTreeSet<String>>) -> Result<Vec<&'a str>, Vec<String>> {
    // `pending[n]` = how many of n's dependencies are not yet resolved.
    let mut pending: BTreeMap<&str, usize> =
        deps.iter().map(|(k, ds)| (*k, ds.iter().filter(|d| deps.contains_key(d.as_str())).count())).collect();
    let mut queue: Vec<&str> = pending.iter().filter(|(_, n)| **n == 0).map(|(k, _)| *k).collect();
    queue.sort();
    let mut order = Vec::new();
    while let Some(node) = queue.pop() {
        order.push(node);
        // Anything that depended on `node` loses one pending edge.
        for (other, ds) in deps {
            if ds.contains(node) {
                if let Some(n) = pending.get_mut(other) {
                    *n -= 1;
                    if *n == 0 {
                        queue.push(other);
                        queue.sort();
                    }
                }
            }
        }
    }
    if order.len() == deps.len() {
        Ok(order)
    } else {
        let cycle: Vec<String> =
            deps.keys().filter(|k| !order.contains(k)).map(|k| k.to_string()).collect();
        Err(cycle)
    }
}

/// Built-in generators (§5). Deterministic sources only.
fn run_generator(name: &str, version: Option<u32>, _input: &Value, raw: &str) -> Result<Value, Diagnostic> {
    // Builtins require an explicit version (§16). Default to 1 if omitted for
    // ergonomics but reject an unknown one.
    if let Some(v) = version {
        if v != 1 {
            return Err(Diagnostic::new(
                Code::UnsupportedAssetVersion,
                format!("builtin generator {name}@{v} not available (have @1)"),
            )
            .with_ref(raw));
        }
    }
    match name {
        "uuid" => Ok(Value::String(uuid::Uuid::new_v4().to_string())),
        "now" => Ok(Value::from(chrono::Utc::now().timestamp())),
        other => Err(Diagnostic::new(
            Code::AssetNotFound,
            format!("unknown builtin generator {other:?}"),
        )
        .with_ref(raw)),
    }
}

// ---------------------------------------------------------------------
// Request / pipeline / mock
// ---------------------------------------------------------------------

fn build_request(
    spec: &RequestSpec,
    _inp: &BuildInputs<'_>,
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    errors: &mut Vec<Diagnostic>,
) -> Option<ResolvedReqParts> {
    let url = interp_string(&spec.url, scopes, sink, "/request/url", errors)?;
    let headers = resolve_headers(&spec.headers, scopes, sink, "/request/headers", errors);
    let query = resolve_headers(&spec.query, scopes, sink, "/request/query", errors);
    let body = build_body(spec.body.as_ref(), _inp, scopes, sink, errors);
    Some(ResolvedReqParts { method: spec.method, url, headers, query, body })
}

fn resolve_headers(
    headers: &[super::model::HeaderSpec],
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    base_path: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<ResolvedHeader> {
    let mut out = Vec::new();
    for (i, h) in headers.iter().enumerate() {
        if !h.enabled {
            continue;
        }
        let path = format!("{base_path}/{i}/value");
        if let Some(value) = interp_string(&h.value, scopes, sink, &path, errors) {
            out.push(ResolvedHeader { name: h.name.clone(), value });
        }
    }
    out
}

fn build_body(
    body: Option<&BodySpec>,
    inp: &BuildInputs<'_>,
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    errors: &mut Vec<Diagnostic>,
) -> ResolvedBody {
    use super::model::BodyType;
    match body {
        None => ResolvedBody::None,
        Some(BodySpec::Inline(b)) => {
            let value = match &b.value {
                Some(v) => match interpolate(v, scopes, sink) {
                    Ok(v) => v,
                    Err(mut d) => {
                        d.instance_path = Some("/request/body/value".to_string());
                        errors.push(d);
                        return ResolvedBody::None;
                    }
                },
                None => return ResolvedBody::None,
            };
            match b.body_type {
                BodyType::Json => ResolvedBody::Json(value),
                BodyType::Text => ResolvedBody::Text(value.as_str().unwrap_or("").to_string()),
                BodyType::Form => ResolvedBody::Form(value_to_form(&value)),
                BodyType::None => ResolvedBody::None,
                BodyType::Multipart | BodyType::Binary => {
                    errors.push(Diagnostic::new(
                        Code::InvalidAssetInput,
                        "multipart/binary bodies are not supported in v1",
                    ).at("/request/body"));
                    ResolvedBody::None
                }
            }
        }
        Some(BodySpec::Ref(r)) => {
            let desc = match inp.resolver.resolve(&r.reference, inp.base_dir) {
                Ok(d) => d,
                Err(mut d) => {
                    d.instance_path = Some("/request/body/ref".to_string());
                    errors.push(d);
                    return ResolvedBody::None;
                }
            };
            match inp.store.resolve(&desc, &[]) {
                Ok(v) => ResolvedBody::Json(v),
                Err(mut d) => {
                    d.instance_path = Some("/request/body/ref".to_string());
                    errors.push(d);
                    ResolvedBody::None
                }
            }
        }
    }
}

fn value_to_form(value: &Value) -> Vec<ResolvedHeader> {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| ResolvedHeader {
                name: k.clone(),
                value: match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn build_pipeline(
    entries: &[PipelineEntry],
    inp: &BuildInputs<'_>,
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    errors: &mut Vec<Diagnostic>,
) -> Vec<ResolvedPipelineEntry> {
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if !e.enabled {
            continue;
        }
        let path = format!("/pipeline/{i}");
        let asset = match inp.resolver.resolve(&e.uses, inp.base_dir) {
            Ok(a) => a,
            Err(mut d) => {
                d.instance_path = Some(path);
                errors.push(d);
                continue;
            }
        };
        let input = match interpolate(&Value::Object(e.with.clone()), scopes, sink) {
            Ok(v) => v,
            Err(mut d) => {
                d.instance_path = Some(format!("{path}/with"));
                errors.push(d);
                continue;
            }
        };
        out.push(ResolvedPipelineEntry { phase: e.phase, asset, input });
    }
    out
}

fn build_mock(
    mock: &MockDef,
    inp: &BuildInputs<'_>,
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    errors: &mut Vec<Diagnostic>,
) -> Option<ResolvedMock> {
    match mock {
        MockDef::Static(m) => {
            let headers = resolve_headers(&m.headers, scopes, sink, "/mock/headers", errors);
            let body = build_body(m.body.as_ref(), inp, scopes, sink, errors);
            Some(ResolvedMock::Static {
                status: m.status,
                headers,
                body,
                delay_ms: m.delay_ms.unwrap_or(0),
            })
        }
        MockDef::Dynamic(m) => {
            let asset = match inp.resolver.resolve(&m.uses, inp.base_dir) {
                Ok(a) => a,
                Err(mut d) => {
                    d.instance_path = Some("/mock/use".to_string());
                    errors.push(d);
                    return None;
                }
            };
            let input = interpolate(&Value::Object(m.with.clone()), scopes, sink).ok()?;
            Some(ResolvedMock::Dynamic { asset, input })
        }
    }
}

fn interp_string(
    s: &str,
    scopes: &Scopes<'_>,
    sink: &mut SecretSink,
    path: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    match interpolate(&Value::String(s.to_string()), scopes, sink) {
        Ok(Value::String(v)) => Some(v),
        Ok(other) => Some(other.to_string()),
        Err(mut d) => {
            d.instance_path = Some(path.to_string());
            errors.push(d);
            None
        }
    }
}
