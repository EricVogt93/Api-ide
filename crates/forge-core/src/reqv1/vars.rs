//! Variable resolution: `${namespace.path}` interpolation over a JSON value
//! tree. See `docs/architecture/request-format-v1.md` §8.
//!
//! Rules:
//! - Namespaces are explicit (`env`, `secret`, `bindings`, `runtime`,
//!   `matrix`); no implicit precedence.
//! - A whole-string single expression preserves the source JSON type.
//! - An expression inside a larger string is coerced: string→itself,
//!   number/bool→JSON text, null→error, object/array→error.
//! - `$${...}` escapes to a literal `${...}`.
//! - A missing variable is an error (strict).
//! - Data-asset content is *not* re-scanned — the caller only runs this over
//!   request-authored strings and asset `with` inputs.

use serde_json::Value;

use super::diag::{Code, Diagnostic};

/// Read-only variable scopes. `bindings`/`matrix`/`runtime` are JSON objects;
/// `env` is JSON; `secret` values are looked up lazily and tracked.
pub struct Scopes<'a> {
    pub env: &'a Value,
    pub bindings: &'a Value,
    pub matrix: &'a Value,
    pub runtime: &'a Value,
    /// Secret provider: name → value. Returns None for a missing secret.
    pub secret: &'a dyn Fn(&str) -> Option<String>,
}

/// Collects the concrete secret values that were interpolated, so the result
/// model and logs can mask them (§8, secret masking).
#[derive(Default)]
pub struct SecretSink {
    pub values: Vec<String>,
}

impl SecretSink {
    fn record(&mut self, v: &str) {
        if !v.is_empty() && !self.values.iter().any(|e| e == v) {
            self.values.push(v.to_string());
        }
    }
}

/// Interpolate every string leaf in `node`. Object keys are never touched.
pub fn interpolate(
    node: &Value,
    scopes: &Scopes<'_>,
    secrets: &mut SecretSink,
) -> Result<Value, Diagnostic> {
    match node {
        Value::String(s) => interpolate_string(s, scopes, secrets),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(interpolate(item, scopes, secrets)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), interpolate(v, scopes, secrets)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Interpolate a single string, honoring whole-expression type preservation
/// and `$$` escaping.
fn interpolate_string(
    s: &str,
    scopes: &Scopes<'_>,
    secrets: &mut SecretSink,
) -> Result<Value, Diagnostic> {
    // Whole-string single expression: `${ ... }` with nothing around it and
    // no escaped `$$` — preserve the resolved type.
    if let Some(expr) = whole_expression(s) {
        return resolve_var(expr, scopes, secrets);
    }

    // Otherwise scan and rebuild as a string, coercing each expression.
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // `$$` -> literal `$` (escape). Only meaningful before `{`, but we
        // collapse any `$$` to `$` so `$${x}` yields `${x}`.
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'$' {
            out.push('$');
            i += 2;
            continue;
        }
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let close = s[i + 2..].find('}').map(|rel| i + 2 + rel);
            let Some(close) = close else {
                return Err(Diagnostic::new(
                    Code::MissingVariable,
                    format!("unterminated variable expression in {s:?}"),
                ));
            };
            let expr = s[i + 2..close].trim();
            let value = resolve_var(expr, scopes, secrets)?;
            out.push_str(&coerce_to_string(&value, expr)?);
            i = close + 1;
            continue;
        }
        // Push one UTF-8 char.
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(Value::String(out))
}

/// If `s` is exactly one `${ expr }` with no escapes and no surrounding text,
/// return `expr`.
fn whole_expression(s: &str) -> Option<&str> {
    let t = s.trim();
    let inner = t.strip_prefix("${")?.strip_suffix('}')?;
    // Reject if the inner part itself contains `}` or `${` (multiple exprs).
    if inner.contains('}') || inner.contains("${") {
        return None;
    }
    Some(inner.trim())
}

/// Resolve one `namespace.path` expression to a JSON value.
fn resolve_var(
    expr: &str,
    scopes: &Scopes<'_>,
    secrets: &mut SecretSink,
) -> Result<Value, Diagnostic> {
    let (ns, path) = match expr.split_once('.') {
        Some((ns, rest)) => (ns, rest),
        None => (expr, ""),
    };

    match ns {
        "secret" => {
            let name = path;
            match (scopes.secret)(name) {
                Some(v) => {
                    secrets.record(&v);
                    Ok(Value::String(v))
                }
                None => Err(missing(expr)),
            }
        }
        "env" => select(scopes.env, path).ok_or_else(|| missing(expr)),
        "bindings" => select(scopes.bindings, path).ok_or_else(|| missing(expr)),
        "matrix" => select(scopes.matrix, path).ok_or_else(|| missing(expr)),
        "runtime" => select(scopes.runtime, path).ok_or_else(|| missing(expr)),
        other => Err(Diagnostic::new(
            Code::UnknownNamespace,
            format!("unknown variable namespace {other:?} in ${{{expr}}}"),
        )),
    }
}

/// Navigate a dotted path (`a.b.c`) into a JSON object. Empty path returns the
/// whole value.
fn select(root: &Value, path: &str) -> Option<Value> {
    if path.is_empty() {
        return Some(root.clone());
    }
    let mut cur = root;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur.clone())
}

fn missing(expr: &str) -> Diagnostic {
    Diagnostic::new(Code::MissingVariable, format!("variable ${{{expr}}} is not defined"))
}

/// Coerce a resolved value for embedding inside a larger string (§8 table).
fn coerce_to_string(value: &Value, expr: &str) -> Result<String, Diagnostic> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Err(Diagnostic::new(
            Code::NullInString,
            format!("cannot interpolate null (${{{expr}}}) into a string"),
        )),
        Value::Array(_) | Value::Object(_) => Err(Diagnostic::new(
            Code::StructuredInString,
            format!("cannot interpolate an object/array (${{{expr}}}) into a string"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(node: Value, bindings: Value, env: Value) -> Result<Value, Diagnostic> {
        let null = Value::Object(Default::default());
        let no_secret = |_: &str| None;
        let scopes = Scopes {
            env: &env,
            bindings: &bindings,
            matrix: &null,
            runtime: &null,
            secret: &no_secret,
        };
        let mut sink = SecretSink::default();
        interpolate(&node, &scopes, &mut sink)
    }

    #[test]
    fn whole_expression_preserves_number_type() {
        let out = run(json!("${bindings.timeout}"), json!({ "timeout": 5000 }), json!({})).unwrap();
        assert_eq!(out, json!(5000));
    }

    #[test]
    fn whole_expression_preserves_object() {
        let user = json!({ "name": "Alice", "email": "a@x" });
        let out = run(json!("${bindings.user}"), json!({ "user": user.clone() }), json!({})).unwrap();
        assert_eq!(out, user);
    }

    #[test]
    fn embedded_expression_coerces_to_string() {
        let out =
            run(json!("id=${bindings.n}!"), json!({ "n": 42 }), json!({})).unwrap();
        assert_eq!(out, json!("id=42!"));
    }

    #[test]
    fn url_from_env_and_binding() {
        let out = run(
            json!("${env.baseUrl}/users/${bindings.id}"),
            json!({ "id": "u-1" }),
            json!({ "baseUrl": "http://x" }),
        )
        .unwrap();
        assert_eq!(out, json!("http://x/users/u-1"));
    }

    #[test]
    fn nested_object_and_array_are_walked() {
        let out = run(
            json!({ "a": ["${bindings.x}", "lit"], "b": { "c": "${bindings.x}" } }),
            json!({ "x": "v" }),
            json!({}),
        )
        .unwrap();
        assert_eq!(out, json!({ "a": ["v", "lit"], "b": { "c": "v" } }));
    }

    #[test]
    fn escape_yields_literal() {
        let out = run(json!("$${keep}"), json!({}), json!({})).unwrap();
        assert_eq!(out, json!("${keep}"));
    }

    #[test]
    fn missing_variable_errors() {
        let err = run(json!("${bindings.nope}"), json!({}), json!({})).unwrap_err();
        assert_eq!(err.code, Code::MissingVariable.as_str());
    }

    #[test]
    fn unknown_namespace_errors() {
        let err = run(json!("${weird.x}"), json!({}), json!({})).unwrap_err();
        assert_eq!(err.code, Code::UnknownNamespace.as_str());
    }

    #[test]
    fn null_into_string_errors() {
        let err = run(json!("x=${bindings.n}"), json!({ "n": null }), json!({})).unwrap_err();
        assert_eq!(err.code, Code::NullInString.as_str());
    }

    #[test]
    fn object_into_string_errors() {
        let err = run(json!("x=${bindings.o}"), json!({ "o": { "a": 1 } }), json!({})).unwrap_err();
        assert_eq!(err.code, Code::StructuredInString.as_str());
    }

    #[test]
    fn null_whole_expression_is_preserved() {
        let out = run(json!("${bindings.n}"), json!({ "n": null }), json!({})).unwrap();
        assert_eq!(out, Value::Null);
    }

    #[test]
    fn secret_is_recorded_for_masking() {
        let env = json!({});
        let bindings = json!({});
        let null = Value::Object(Default::default());
        let secret = |name: &str| (name == "apiToken").then(|| "s3cr3t".to_string());
        let scopes = Scopes {
            env: &env,
            bindings: &bindings,
            matrix: &null,
            runtime: &null,
            secret: &secret,
        };
        let mut sink = SecretSink::default();
        let out =
            interpolate(&json!("Bearer ${secret.apiToken}"), &scopes, &mut sink).unwrap();
        assert_eq!(out, json!("Bearer s3cr3t"));
        assert_eq!(sink.values, vec!["s3cr3t".to_string()]);
    }
}
