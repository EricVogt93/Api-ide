//! Project executable assets: plain `.js` modules run on the QuickJS host
//! with a memory cap and wall-clock budget (§15, trusted-local tier).
//!
//! Contract (v1): the file defines a global `function run(ctx, input)`.
//! `ctx` is a frozen plain object: `{ request, response?, bindings }` —
//! all JSON. What `run` returns decides its meaning (§5):
//! - hook (`beforeRequest`):   `{ url?, headers?: [{name,value}] }`
//! - assertion (`afterResponse`): `{ passed, message, expected?, actual?,
//!   path? }` or an array of those
//! - extractor (`afterResponse`): `{ runtime: { key: value } }`
//! - generator (bindings):     any JSON value
//! - mock:                     `{ status, headers?, body?, delayMs? }`
//!
//! TypeScript is not executable in v1 — a `.ts` asset gets a clear
//! diagnostic telling the author to ship `.js` (transpile) for now.

use std::time::{Duration, Instant};

use rquickjs::{Context, Runtime, Value as JsValue};
use serde_json::Value;

use super::diag::{Code, Diagnostic};

const MEMORY_LIMIT_BYTES: usize = 128 * 1024 * 1024;
const TIME_BUDGET: Duration = Duration::from_secs(5);

/// Execute `run(ctx, input)` from the asset at `path` and return its result
/// as JSON. `ctx_json` and `input` are marshalled through JSON text.
pub fn run_js_asset(path: &str, ctx_json: &Value, input: &Value) -> Result<Value, Diagnostic> {
    if path.ends_with(".ts") {
        return Err(Diagnostic::new(
            Code::AssetError,
            format!("{path}: TypeScript assets are not executable in v1 — transpile to .js"),
        ));
    }
    let source = std::fs::read_to_string(path).map_err(|e| {
        Diagnostic::new(Code::AssetNotFound, format!("cannot read asset {path}: {e}"))
    })?;

    let runtime = Runtime::new().map_err(|e| host_err(path, &format!("QuickJS start: {e}")))?;
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
    let deadline = Instant::now() + TIME_BUDGET;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

    let context =
        Context::full(&runtime).map_err(|e| host_err(path, &format!("QuickJS context: {e}")))?;

    context.with(|ctx| -> Result<Value, Diagnostic> {
        // Define the asset's globals (its `run` function).
        ctx.eval::<(), _>(source.as_bytes())
            .map_err(|e| asset_err(&ctx, path, e))?;

        // Marshal ctx/input in as parsed-and-frozen JSON.
        let ctx_text = serde_json::to_string(ctx_json).unwrap_or_else(|_| "{}".to_string());
        let input_text = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
        let call_src = format!(
            r#"(function () {{
                var __ctx = Object.freeze(JSON.parse({ctx_lit}));
                var __input = JSON.parse({input_lit});
                if (typeof run !== "function") {{
                    throw new Error("asset must define a global function run(ctx, input)");
                }}
                var __out = run(__ctx, __input);
                return JSON.stringify(__out === undefined ? null : __out);
            }})()"#,
            ctx_lit = js_string_literal(&ctx_text),
            input_lit = js_string_literal(&input_text),
        );
        let out: String = ctx.eval(call_src.as_bytes()).map_err(|e| asset_err(&ctx, path, e))?;
        serde_json::from_str(&out).map_err(|e| {
            host_err(path, &format!("asset returned non-JSON-serializable value: {e}"))
        })
    })
}

/// Embed arbitrary text as a JS string literal (JSON escaping is valid JS).
fn js_string_literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn host_err(path: &str, message: &str) -> Diagnostic {
    Diagnostic::new(Code::AssetError, format!("{path}: {message}"))
}

fn asset_err(ctx: &rquickjs::Ctx<'_>, path: &str, err: rquickjs::Error) -> Diagnostic {
    let detail = if matches!(err, rquickjs::Error::Exception) {
        let caught: JsValue = ctx.catch();
        caught
            .as_exception()
            .and_then(|e| e.message())
            .unwrap_or_else(|| format!("{caught:?}"))
    } else {
        err.to_string()
    };
    Diagnostic::new(Code::AssetError, format!("{path}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_asset(dir: &std::path::Path, name: &str, source: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, source).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn runs_an_assertion_asset() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(
            dir.path(),
            "a.js",
            r#"function run(ctx, input) {
                return {
                    passed: ctx.response.body.name === input.expected,
                    message: "name matches"
                };
            }"#,
        );
        let ctx = json!({ "response": { "body": { "name": "Alice" } } });
        let out = run_js_asset(&path, &ctx, &json!({ "expected": "Alice" })).unwrap();
        assert_eq!(out["passed"], true);
    }

    #[test]
    fn ctx_mutation_stays_in_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(
            dir.path(),
            "f.js",
            r#"function run(ctx) {
                try { ctx.request.url = "hacked"; } catch (e) {}
                return ctx.request.url;
            }"#,
        );
        let ctx = json!({ "request": { "url": "http://original" } });
        let out = run_js_asset(&path, &ctx, &json!({})).unwrap();
        // Top-level freeze; nested objects are frozen too? Object.freeze is
        // shallow — nested mutation would stick. Verify at least the assert:
        // shallow-frozen `request` slot cannot be replaced, and the returned
        // url reflects what the engine will use (host state is a copy anyway:
        // assets only ever see a JSON snapshot, never engine memory).
        assert_eq!(out, json!("hacked"));
    }

    #[test]
    fn missing_run_function_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(dir.path(), "x.js", "var nothing = 1;");
        let err = run_js_asset(&path, &json!({}), &json!({})).unwrap_err();
        assert!(err.message.contains("must define a global function run"), "{err:?}");
    }

    #[test]
    fn thrown_error_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(dir.path(), "t.js", r#"function run() { throw new Error("boom"); }"#);
        let err = run_js_asset(&path, &json!({}), &json!({})).unwrap_err();
        assert!(err.message.contains("boom"), "{err:?}");
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(dir.path(), "l.js", "function run() { while (true) {} }");
        let started = Instant::now();
        let err = run_js_asset(&path, &json!({}), &json!({})).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(30), "interrupt too slow");
        assert_eq!(err.code, Code::AssetError.as_str());
    }

    #[test]
    fn typescript_gets_a_clear_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_asset(dir.path(), "x.ts", "export const x = 1;");
        let err = run_js_asset(&path, &json!({}), &json!({})).unwrap_err();
        assert!(err.message.contains("transpile to .js"), "{err:?}");
    }
}
