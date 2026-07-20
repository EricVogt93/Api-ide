//! Sandboxed JavaScript scripting via QuickJS ([`rquickjs`]), exposing the
//! same `req`/`res`/`vars`/`assert`/`log`/helpers host API as the Rhai
//! engine in [`super::engine`] — just from JavaScript instead of Rhai.
//!
//! Every call to [`JsEngine::run_pre`]/[`JsEngine::run_post`]/
//! [`JsEngine::run_hook`] builds a fresh `Runtime`/`Context` (QuickJS
//! contexts are cheap to create and this keeps every execution fully
//! isolated, with no shared mutable state between calls). Host state
//! (`req`, `vars`, captured log lines, recorded assertions) is shared with
//! the registered JS callbacks through `Rc<RefCell<..>>` — safe because a
//! single `Context::with` call, and everything nested inside it, runs on one
//! thread with no reentrancy into the callbacks themselves.
//!
//! The low-level callbacks registered from Rust use a leading `__` (e.g.
//! `__reqGetUrl`); a small JS prelude, evaluated before the user's script,
//! wraps them into the ergonomic `req.url`/`res.status`/`vars.get(..)`
//! surface scripts actually see.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rquickjs::{Context, Ctx, Function, Runtime, Value};

use crate::assert::AssertionOutcome;
use crate::exec::{ExecutionResult, ResolvedBody, ResolvedRequest};

use super::{ScriptOutput, VarMutation};

/// Memory ceiling for a single script execution.
const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Wall-clock budget for a single script execution; enforced via the
/// QuickJS interrupt handler, which the engine polls periodically while
/// running bytecode (so even a tight `while (true) {}` gets caught).
const TIME_BUDGET: Duration = Duration::from_secs(2);

/// A sandboxed JavaScript engine for pre-request / post-response / suite
/// hook scripting. Holds no state of its own — every call is fully
/// self-contained — so it's `Send`/`Sync` for free and cheap to construct.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsEngine;

impl JsEngine {
    pub fn new() -> Self {
        Self
    }

    /// Run a pre-request script. `req` is mutated in place through the
    /// script's `req.*` calls; `vars` is the read/write variable scope
    /// visible to the script as `vars.*`. No `assert`/`test` in scope.
    pub fn run_pre(
        &self,
        script: &str,
        req: &mut ResolvedRequest,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        let req_state = Rc::new(RefCell::new(req.clone()));
        let vars_state = Rc::new(RefCell::new(vars.clone()));
        let var_mutations = Rc::new(RefCell::new(Vec::new()));
        // Postman-style `pm.test` may run in pre-request scripts too.
        let assertions = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::new(RefCell::new(Vec::new()));

        let error = run_sandboxed(
            |ctx| {
                install_helpers(ctx)?;
                install_log(ctx, &log)?;
                install_vars(ctx, &vars_state, &var_mutations)?;
                install_req(ctx, &req_state)?;
                super::pm::install_pm(ctx, &assertions, false)?;
                Ok(())
            },
            script,
        );

        *req = req_state.borrow().clone();

        let log = log.borrow().clone();
        let assertions = assertions.borrow().clone();
        let var_mutations = var_mutations.borrow().clone();
        ScriptOutput {
            log,
            error,
            assertions,
            var_mutations,
        }
    }

    /// Run a post-response script. `res` is read-only; `assert`/`test`
    /// calls are recorded into [`ScriptOutput::assertions`] and never abort
    /// the script. Also used by the runner for `afterEach`/`afterAll` suite
    /// hooks, which get the same `res`/`vars`/`assert` surface.
    pub fn run_post(
        &self,
        script: &str,
        res: &ExecutionResult,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        let res_state = Rc::new(res.clone());
        let vars_state = Rc::new(RefCell::new(vars.clone()));
        let var_mutations = Rc::new(RefCell::new(Vec::new()));
        let assertions = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::new(RefCell::new(Vec::new()));

        let error = run_sandboxed(
            |ctx| {
                install_helpers(ctx)?;
                install_log(ctx, &log)?;
                install_vars(ctx, &vars_state, &var_mutations)?;
                install_assertions(ctx, &assertions)?;
                install_res(ctx, &res_state)?;
                super::pm::install_pm(ctx, &assertions, true)?;
                Ok(())
            },
            script,
        );

        let log = log.borrow().clone();
        let assertions = assertions.borrow().clone();
        let var_mutations = var_mutations.borrow().clone();
        ScriptOutput {
            log,
            error,
            assertions,
            var_mutations,
        }
    }

    /// Run a suite lifecycle hook (`beforeAll`/`beforeEach`). Exposes
    /// `vars`/`log`/`assert`/`test`/helpers, but no `req`/`res` — hooks that
    /// need the response (`afterEach`/`afterAll`) go through
    /// [`Self::run_post`] instead.
    pub fn run_hook(&self, script: &str, vars: &BTreeMap<String, String>) -> ScriptOutput {
        let vars_state = Rc::new(RefCell::new(vars.clone()));
        let var_mutations = Rc::new(RefCell::new(Vec::new()));
        let assertions = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::new(RefCell::new(Vec::new()));

        let error = run_sandboxed(
            |ctx| {
                install_helpers(ctx)?;
                install_log(ctx, &log)?;
                install_vars(ctx, &vars_state, &var_mutations)?;
                install_assertions(ctx, &assertions)?;
                super::pm::install_pm(ctx, &assertions, false)?;
                Ok(())
            },
            script,
        );

        let log = log.borrow().clone();
        let assertions = assertions.borrow().clone();
        let var_mutations = var_mutations.borrow().clone();
        ScriptOutput {
            log,
            error,
            assertions,
            var_mutations,
        }
    }
}

/// Build a fresh sandboxed `Runtime`/`Context`, let `install` register
/// whatever host bindings this call needs (each install also `eval`s its
/// own small JS prelude), then run `script`. Returns `None` on success or a
/// human-readable error (compile or runtime, including a sandbox-limit
/// trip) on failure. Never panics: every fallible step is folded into the
/// returned error message instead.
fn run_sandboxed(
    install: impl FnOnce(&Ctx) -> rquickjs::Result<()>,
    script: &str,
) -> Option<String> {
    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return Some(format!("script engine error: failed to start QuickJS: {e}")),
    };
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);

    let deadline = Instant::now() + TIME_BUDGET;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

    let context = match Context::full(&runtime) {
        Ok(ctx) => ctx,
        Err(e) => {
            return Some(format!(
                "script engine error: failed to create context: {e}"
            ))
        }
    };

    context.with(|ctx| {
        if let Err(e) = install(&ctx) {
            return Some(format!("script engine error: {}", describe_error(&ctx, e)));
        }
        match ctx.eval::<Value<'_>, _>(script) {
            Ok(_) => None,
            Err(e) => Some(describe_error(&ctx, e)),
        }
    })
}

/// Turn a `rquickjs::Error` into a human-readable message, pulling the
/// message (and, if present, a source line) out of the thrown JS exception.
fn describe_error(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    if !matches!(err, rquickjs::Error::Exception) {
        return format!("script error: {err}");
    }
    let value = ctx.catch();
    let Some(exception) = value.as_exception() else {
        return format!("script error: {value:?}");
    };
    let name = exception
        .as_object()
        .get::<_, Option<String>>("name")
        .ok()
        .flatten();
    let message = exception
        .message()
        .unwrap_or_else(|| "unknown error".to_string());
    let label = if name.as_deref() == Some("SyntaxError") {
        "script compile error"
    } else {
        "script error"
    };
    match exception.stack().as_deref().and_then(stack_line) {
        Some(line) => format!("{label}: {message} (line {line})"),
        None => format!("{label}: {message}"),
    }
}

/// Best-effort extraction of a `:<line>:<col>` source position from a
/// QuickJS stack trace string.
fn stack_line(stack: &str) -> Option<u32> {
    let re = regex::Regex::new(r":(\d+):\d+\)?").ok()?;
    re.captures(stack)?.get(1)?.as_str().parse().ok()
}

// ---------------------------------------------------------------------
// Host bindings
// ---------------------------------------------------------------------

/// `uuid()`, `timestamp()`, `base64Encode(s)`, `base64Decode(s)` — no
/// per-execution state needed, so these are plain functions under their
/// final names (no wrapper prelude required).
fn install_helpers(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    globals.set(
        "uuid",
        Function::new(ctx.clone(), || uuid::Uuid::new_v4().to_string())?,
    )?;
    globals.set(
        "timestamp",
        Function::new(ctx.clone(), || chrono::Utc::now().timestamp())?,
    )?;
    globals.set(
        "base64Encode",
        Function::new(ctx.clone(), |s: String| BASE64.encode(s.as_bytes()))?,
    )?;
    globals.set(
        "base64Decode",
        Function::new(ctx.clone(), |s: String| -> String {
            match BASE64.decode(s.as_bytes()) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => String::new(),
            }
        })?,
    )?;
    Ok(())
}

/// `log(msg)` and `console.log(...)`, both feeding the same shared buffer.
fn install_log(ctx: &Ctx<'_>, log: &Rc<RefCell<Vec<String>>>) -> rquickjs::Result<()> {
    let sink = log.clone();
    ctx.globals().set(
        "__hostLog",
        Function::new(ctx.clone(), move |msg: String| {
            sink.borrow_mut().push(msg);
        })?,
    )?;
    ctx.eval::<(), _>(
        r#"
            function __toLogString(x) {
                if (typeof x === "undefined") return "undefined";
                if (x === null) return "null";
                if (typeof x === "string") return x;
                try { return JSON.stringify(x); } catch (e) { return String(x); }
            }
            function log(msg) { __hostLog(__toLogString(msg)); }
            var console = {
                log: function () {
                    var parts = [];
                    for (var i = 0; i < arguments.length; i++) parts.push(__toLogString(arguments[i]));
                    __hostLog(parts.join(" "));
                }
            };
        "#,
    )
}

/// `vars.get(name)` / `vars.set(name, value)` / `vars.unset(name)`.
fn install_vars(
    ctx: &Ctx<'_>,
    values: &Rc<RefCell<BTreeMap<String, String>>>,
    mutations: &Rc<RefCell<Vec<VarMutation>>>,
) -> rquickjs::Result<()> {
    let for_get = values.clone();
    ctx.globals().set(
        "__varsGet",
        Function::new(ctx.clone(), move |name: String| -> Option<String> {
            for_get.borrow().get(&name).cloned()
        })?,
    )?;
    let (for_set_values, for_set_mutations) = (values.clone(), mutations.clone());
    ctx.globals().set(
        "__varsSet",
        Function::new(ctx.clone(), move |name: String, value: String| {
            for_set_values
                .borrow_mut()
                .insert(name.clone(), value.clone());
            for_set_mutations
                .borrow_mut()
                .push(VarMutation::Set(name, value));
        })?,
    )?;
    let (for_unset_values, for_unset_mutations) = (values.clone(), mutations.clone());
    ctx.globals().set(
        "__varsUnset",
        Function::new(ctx.clone(), move |name: String| {
            for_unset_values.borrow_mut().remove(&name);
            for_unset_mutations
                .borrow_mut()
                .push(VarMutation::Unset(name));
        })?,
    )?;
    ctx.eval::<(), _>(
        r#"
            var vars = {
                get: function (name) { return __varsGet(String(name)); },
                set: function (name, value) { __varsSet(String(name), String(value)); },
                unset: function (name) { __varsUnset(String(name)); }
            };
        "#,
    )
}

/// `assert(cond, message)` / `test(name, cond)`.
fn install_assertions(
    ctx: &Ctx<'_>,
    assertions: &Rc<RefCell<Vec<AssertionOutcome>>>,
) -> rquickjs::Result<()> {
    let for_assert = assertions.clone();
    ctx.globals().set(
        "__hostAssert",
        Function::new(ctx.clone(), move |passed: bool, message: String| {
            for_assert.borrow_mut().push(AssertionOutcome {
                summary: message,
                passed,
                message: None,
            });
        })?,
    )?;
    let for_test = assertions.clone();
    ctx.globals().set(
        "__hostTest",
        Function::new(ctx.clone(), move |name: String, passed: bool| {
            for_test.borrow_mut().push(AssertionOutcome {
                summary: name,
                passed,
                message: None,
            });
        })?,
    )?;
    ctx.eval::<(), _>(
        r#"
            function assert(cond, message) { __hostAssert(!!cond, message === undefined ? "" : String(message)); }
            function test(name, cond) { __hostTest(String(name), !!cond); }
        "#,
    )
}

/// `req.url` (get/set), `req.method` (read), `req.setHeader`/`getHeader`/
/// `removeHeader`, `req.setBodyText`/`bodyText`.
fn install_req(ctx: &Ctx<'_>, req: &Rc<RefCell<ResolvedRequest>>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    let r = req.clone();
    globals.set(
        "__reqGetUrl",
        Function::new(ctx.clone(), move || r.borrow().url.clone())?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqSetUrl",
        Function::new(ctx.clone(), move |v: String| r.borrow_mut().url = v)?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqGetMethod",
        Function::new(ctx.clone(), move || r.borrow().method.as_str().to_string())?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqSetHeader",
        Function::new(ctx.clone(), move |name: String, value: String| {
            let mut req = r.borrow_mut();
            match req
                .headers
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(&name))
            {
                Some(existing) => existing.1 = value,
                None => req.headers.push((name, value)),
            }
        })?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqGetHeader",
        Function::new(ctx.clone(), move |name: String| -> Option<String> {
            r.borrow()
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&name))
                .map(|(_, v)| v.clone())
        })?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqRemoveHeader",
        Function::new(ctx.clone(), move |name: String| {
            r.borrow_mut()
                .headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
        })?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqSetBodyText",
        Function::new(ctx.clone(), move |text: String| {
            let mut req = r.borrow_mut();
            let content_type = match &req.body {
                ResolvedBody::Bytes { content_type, .. } => content_type.clone(),
                _ => None,
            };
            req.body = ResolvedBody::Bytes {
                content_type,
                data: text.into_bytes(),
            };
        })?,
    )?;
    let r = req.clone();
    globals.set(
        "__reqGetBodyText",
        Function::new(ctx.clone(), move || -> String {
            match &r.borrow().body {
                ResolvedBody::Bytes { data, .. } => String::from_utf8_lossy(data).into_owned(),
                _ => String::new(),
            }
        })?,
    )?;

    ctx.eval::<(), _>(
        r#"
            var req = {
                get url() { return __reqGetUrl(); },
                set url(v) { __reqSetUrl(String(v)); },
                get method() { return __reqGetMethod(); },
                setHeader: function (n, v) { __reqSetHeader(String(n), String(v)); },
                getHeader: function (n) { return __reqGetHeader(String(n)); },
                removeHeader: function (n) { __reqRemoveHeader(String(n)); },
                setBodyText: function (t) { __reqSetBodyText(String(t)); },
                get bodyText() { return __reqGetBodyText(); }
            };
        "#,
    )
}

/// `res.status`, `res.bodyText`, `res.timeMs`, `res.header(name)`,
/// `res.json()`.
fn install_res<'js>(ctx: &Ctx<'js>, res: &Rc<ExecutionResult>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    let r = res.clone();
    globals.set(
        "__resGetStatus",
        Function::new(ctx.clone(), move || i64::from(r.status))?,
    )?;
    let r = res.clone();
    globals.set(
        "__resGetStatusText",
        Function::new(ctx.clone(), move || r.status_text.clone())?,
    )?;
    let r = res.clone();
    globals.set(
        "__resGetBodyText",
        Function::new(ctx.clone(), move || r.text().into_owned())?,
    )?;
    let r = res.clone();
    globals.set(
        "__resGetTimeMs",
        // `Duration::as_millis` returns u128; response times never come
        // close to overflowing an i64 worth of milliseconds.
        Function::new(ctx.clone(), move || r.timing.total.as_millis() as i64)?,
    )?;
    let r = res.clone();
    globals.set(
        "__resGetHeader",
        Function::new(ctx.clone(), move |name: String| -> Option<String> {
            r.header(&name).map(|v| v.to_string())
        })?,
    )?;
    let r = res.clone();
    globals.set(
        "__resJson",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                match ctx.json_parse(r.body.clone()) {
                    Ok(v) => Ok(v),
                    Err(_) => Ok(Value::new_undefined(ctx)),
                }
            },
        )?,
    )?;

    ctx.eval::<(), _>(
        r#"
            var res = {
                get status() { return __resGetStatus(); },
                get statusText() { return __resGetStatusText(); },
                get bodyText() { return __resGetBodyText(); },
                get timeMs() { return __resGetTimeMs(); },
                header: function (n) { return __resGetHeader(String(n)); },
                json: function () { return __resJson(); }
            };
        "#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{Hop, Sizes, TimingBreakdown};
    use crate::model::Method;
    use std::time::Duration as StdDuration;

    fn empty_vars() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn sample_result(status: u16, body: &str) -> ExecutionResult {
        ExecutionResult {
            status,
            status_text: "OK".into(),
            http_version: "HTTP/1.1".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
            timing: TimingBreakdown {
                dns: None,
                connect_tls: None,
                connect: None,
                tls: None,
                ttfb: StdDuration::from_millis(12),
                download: StdDuration::from_millis(3),
                total: StdDuration::from_millis(42),
            },
            size: Sizes::default(),
            effective_url: "https://example.test/thing".into(),
            redirect_chain: Vec::<Hop>::new(),
            cookies_set: Vec::new(),
            executed_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn pre_script_mutates_url_headers_body_and_records_vars() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test/old");
        req.headers.push(("X-Existing".into(), "1".into()));

        let script = r#"
            req.url = req.url + "?patched=1";
            req.setHeader("X-New", "hello");
            req.setHeader("X-Existing", "2");
            req.removeHeader("X-Existing");
            req.setBodyText("payload");
            vars.set("token", "abc123");
            log("done");
        "#;

        let out = engine.run_pre(script, &mut req, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(req.url, "https://example.test/old?patched=1");
        assert_eq!(req.header("X-New"), Some("hello"));
        assert!(
            req.header("X-Existing").is_none(),
            "header should have been removed"
        );
        match &req.body {
            ResolvedBody::Bytes { data, .. } => assert_eq!(data, b"payload"),
            other => panic!("expected Bytes body, got {other:?}"),
        }
        assert_eq!(
            out.var_mutations,
            vec![VarMutation::Set("token".to_string(), "abc123".to_string())]
        );
        assert_eq!(out.log, vec!["done".to_string()]);
        assert!(out.assertions.is_empty());
    }

    #[test]
    fn pre_script_reads_existing_vars() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");
        let mut vars = BTreeMap::new();
        vars.insert("base".to_string(), "https://api.test".to_string());

        let out = engine.run_pre(r#"req.url = vars.get("base") + "/path";"#, &mut req, &vars);

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(req.url, "https://api.test/path");
    }

    #[test]
    fn pre_script_missing_var_is_undefined() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                var v = vars.get("missing");
                log(typeof v === "undefined" ? "was-undefined" : "was-set");
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["was-undefined".to_string()]);
    }

    #[test]
    fn post_script_records_pass_and_fail_assertions() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{"ok":true}"#);

        let script = r#"
            assert(res.status == 200, "status is 200");
            assert(res.status == 404, "status is 404");
            test("has ok field", res.json().ok === true);
        "#;

        let out = engine.run_post(script, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.assertions.len(), 3);
        assert!(out.assertions[0].passed);
        assert_eq!(out.assertions[0].summary, "status is 200");
        assert!(!out.assertions[1].passed);
        assert_eq!(out.assertions[1].summary, "status is 404");
        assert!(out.assertions[2].passed);
        assert_eq!(out.assertions[2].summary, "has ok field");
    }

    #[test]
    fn post_script_reads_json_body_text_and_headers() {
        let engine = JsEngine::new();
        let res = sample_result(201, r#"{"name":"forge","count":3}"#);

        let script = r#"
            var j = res.json();
            log(j.name);
            log(j.count.toString());
            log(res.bodyText);
            log(res.header("Content-Type"));
            log(res.timeMs.toString());
        "#;

        let out = engine.run_post(script, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.log,
            vec![
                "forge".to_string(),
                "3".to_string(),
                r#"{"name":"forge","count":3}"#.to_string(),
                "application/json".to_string(),
                "42".to_string(),
            ]
        );
    }

    #[test]
    fn res_json_is_undefined_for_non_json_body() {
        let engine = JsEngine::new();
        let res = sample_result(200, "not json");

        let out = engine.run_post(
            r#"log(typeof res.json() === "undefined" ? "undefined" : "not-undefined");"#,
            &res,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["undefined".to_string()]);
    }

    #[test]
    fn post_script_vars_set_is_captured() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{"id":"xyz"}"#);

        let out = engine.run_post(r#"vars.set("id", res.json().id);"#, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.var_mutations,
            vec![VarMutation::Set("id".to_string(), "xyz".to_string())]
        );
    }

    #[test]
    fn console_log_capture_preserves_order_and_joins_args() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                log("one");
                console.log("two", 3);
                log("four");
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.log,
            vec!["one".to_string(), "two 3".to_string(), "four".to_string()]
        );
    }

    #[test]
    fn hook_script_has_no_req_or_res_binding() {
        let engine = JsEngine::new();

        let out = engine.run_hook("req;", &empty_vars());
        assert!(
            out.error.is_some(),
            "expected `req` to be unavailable in a hook script"
        );

        let out = engine.run_hook("res;", &empty_vars());
        assert!(
            out.error.is_some(),
            "expected `res` to be unavailable in a hook script"
        );
    }

    #[test]
    fn hook_script_sets_vars_and_records_assertions() {
        let engine = JsEngine::new();

        let out = engine.run_hook(
            r#"
                vars.set("suite", "started");
                assert(vars.get("suite") === "started", "suite var visible");
                log("hook ran");
            "#,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.var_mutations,
            vec![VarMutation::Set("suite".to_string(), "started".to_string())]
        );
        assert_eq!(out.assertions.len(), 1);
        assert!(out.assertions[0].passed);
        assert_eq!(out.log, vec!["hook ran".to_string()]);
    }

    #[test]
    fn infinite_loop_is_terminated_by_time_budget() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let started = Instant::now();
        let out = engine.run_pre("while (true) {}", &mut req, &empty_vars());
        let elapsed = started.elapsed();

        assert!(
            out.error.is_some(),
            "expected the runaway script to be interrupted"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "took too long to interrupt: {elapsed:?}"
        );
    }

    #[test]
    fn compile_error_is_reported() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("let x = ;", &mut req, &empty_vars());

        let err = out.error.expect("expected a compile error");
        assert!(
            err.starts_with("script compile error:"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_error_is_reported() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("undefinedFunction();", &mut req, &empty_vars());

        let err = out.error.expect("expected a runtime error");
        assert!(err.starts_with("script error:"), "unexpected error: {err}");
    }

    #[test]
    fn base64_roundtrip_helpers() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                var encoded = base64Encode("hello world");
                log(encoded);
                log(base64Decode(encoded));
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log[0], "aGVsbG8gd29ybGQ=");
        assert_eq!(out.log[1], "hello world");
    }

    #[test]
    fn uuid_has_expected_format() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("log(uuid());", &mut req, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log.len(), 1);
        let id = &out.log[0];
        assert_eq!(id.len(), 36);
        assert!(uuid::Uuid::parse_str(id).is_ok(), "not a valid uuid: {id}");
    }

    #[test]
    fn timestamp_returns_current_epoch_seconds() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let before = chrono::Utc::now().timestamp();
        let out = engine.run_pre("log(timestamp().toString());", &mut req, &empty_vars());
        let after = chrono::Utc::now().timestamp();

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        let ts: i64 = out.log[0].parse().expect("timestamp should be an integer");
        assert!(ts >= before - 1 && ts <= after + 1);
    }

    #[test]
    fn get_header_returns_undefined_when_missing() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                var v = req.getHeader("X-Missing");
                log(typeof v === "undefined" ? "undefined" : v);
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["undefined".to_string()]);
    }

    #[test]
    fn pm_test_and_expect_record_named_outcomes() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{"value":100,"tags":["a","b"]}"#);

        let script = r#"
            pm.test("Status code is 200", function () {
                pm.response.to.have.status(200);
            });
            pm.test("body checks out", function () {
                var jsonData = pm.response.json();
                pm.expect(jsonData.value).to.eql(100);
                pm.expect(jsonData.tags).to.have.lengthOf(2);
                pm.expect(jsonData.tags).to.include("a");
                pm.expect(pm.response.responseTime).to.be.below(200);
            });
            pm.test("this one fails", function () {
                pm.expect(pm.response.code).to.equal(404);
            });
        "#;

        let out = engine.run_post(script, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.assertions.len(), 3);
        assert!(out.assertions[0].passed);
        assert_eq!(out.assertions[0].summary, "Status code is 200");
        assert!(out.assertions[1].passed, "{:?}", out.assertions[1]);
        assert!(!out.assertions[2].passed);
        assert!(
            out.assertions[2]
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("expected 200 to equal 404"),
            "{:?}",
            out.assertions[2].message
        );
    }

    #[test]
    fn pm_expect_negation_deep_equal_and_property() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{"user":{"name":"eric","roles":["admin"]}}"#);

        let script = r#"
            pm.test("chai surface", function () {
                var u = pm.response.json().user;
                pm.expect(u).to.be.an("object");
                pm.expect(u).to.have.property("name", "eric");
                pm.expect(u).to.not.have.property("password");
                pm.expect(u.roles).to.eql(["admin"]);
                pm.expect(u.name).to.match(/^er/);
                pm.expect("").to.be.empty;
                pm.expect(null).to.be.null;
                pm.expect(u.name).to.be.oneOf(["eric", "bob"]);
            });
        "#;

        let out = engine.run_post(script, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.assertions.len(), 1);
        assert!(out.assertions[0].passed, "{:?}", out.assertions[0]);
    }

    #[test]
    fn pm_variable_scopes_share_forges_runtime_vars() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{"token":"t-123"}"#);
        let mut vars = BTreeMap::new();
        vars.insert("existing".to_string(), "yes".to_string());

        let script = r#"
            pm.environment.set("token", pm.response.json().token);
            pm.globals.unset("existing");
            pm.test("scopes alias the same store", function () {
                pm.expect(pm.variables.get("token")).to.equal("t-123");
                pm.expect(pm.collectionVariables.has("existing")).to.be.false;
                pm.expect(pm.globals.has("missing")).to.be.false;
            });
            log(pm.variables.replaceIn("tok={{token}} keep={{missing}}"));
        "#;

        let out = engine.run_post(script, &res, &vars);

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert!(out
            .var_mutations
            .contains(&VarMutation::Set("token".to_string(), "t-123".to_string())));
        assert!(out
            .var_mutations
            .contains(&VarMutation::Unset("existing".to_string())));
        assert!(out.assertions[0].passed, "{:?}", out.assertions[0]);
        assert_eq!(out.log, vec!["tok=t-123 keep={{missing}}".to_string()]);
    }

    #[test]
    fn pm_response_status_header_and_class_helpers() {
        let engine = JsEngine::new();
        let res = sample_result(200, r#"{}"#);

        let script = r#"
            pm.test("status text and headers", function () {
                pm.response.to.have.status("OK");
                pm.response.to.have.header("Content-Type");
                pm.expect(pm.response.headers.get("Content-Type")).to.equal("application/json");
                pm.expect(pm.response.headers.get("X-Missing")).to.be.null;
                pm.response.to.be.ok;
            });
        "#;

        let out = engine.run_post(script, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert!(out.assertions[0].passed, "{:?}", out.assertions[0]);
    }

    #[test]
    fn pm_request_is_usable_in_pre_scripts_and_response_is_not() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Post, "https://example.test/x");

        let script = r#"
            pm.request.headers.upsert({ key: "X-From-Pm", value: "1" });
            pm.test("request surface", function () {
                pm.expect(pm.request.method).to.equal("POST");
                pm.expect(pm.request.headers.get("X-From-Pm")).to.equal("1");
            });
            pm.test("response must not exist here", function () {
                var threw = false;
                try { pm.response; } catch (e) { threw = true; }
                pm.expect(threw).to.be.true;
            });
        "#;

        let out = engine.run_pre(script, &mut req, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(req.header("X-From-Pm"), Some("1"));
        assert!(
            out.assertions.iter().all(|a| a.passed),
            "{:?}",
            out.assertions
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pm_send_request_performs_a_real_get_and_post() {
        use wiremock::matchers::{body_string, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Kind", "token")
                    .set_body_string(r#"{"token":"tok-1"}"#),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/audit"))
            .and(header("X-From", "script"))
            .and(body_string(r#"{"seen":true}"#))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        let uri = server.uri();
        let script = format!(
            r#"
                pm.sendRequest("{uri}/token", function (err, response) {{
                    pm.test("GET roundtrip", function () {{
                        pm.expect(err).to.be.null;
                        pm.expect(response.code).to.equal(200);
                        pm.expect(response.json().token).to.equal("tok-1");
                        pm.expect(response.headers.get("X-Kind")).to.equal("token");
                        pm.expect(response.responseTime).to.be.a("number");
                    }});
                    pm.environment.set("token", response.json().token);
                }});
                pm.sendRequest({{
                    url: "{uri}/audit",
                    method: "POST",
                    header: [{{ key: "X-From", value: "script" }}],
                    body: {{ mode: "raw", raw: JSON.stringify({{ seen: true }}) }}
                }}, function (err, response) {{
                    pm.test("POST roundtrip", function () {{
                        pm.expect(err).to.be.null;
                        pm.expect(response.code).to.equal(201);
                    }});
                }});
            "#
        );

        // The script blocks its thread while waiting for the reply, so keep
        // it off this runtime's core workers.
        let res = sample_result(200, "{}");
        let out = tokio::task::spawn_blocking(move || {
            JsEngine::new().run_post(&script, &res, &BTreeMap::new())
        })
        .await
        .expect("script task");

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.assertions.len(), 2, "{:?}", out.assertions);
        assert!(
            out.assertions.iter().all(|a| a.passed),
            "{:?}",
            out.assertions
        );
        assert!(out
            .var_mutations
            .contains(&VarMutation::Set("token".to_string(), "tok-1".to_string())));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pm_send_request_reports_transport_errors_via_the_callback() {
        let out = tokio::task::spawn_blocking(|| {
            let res = sample_result(200, "{}");
            JsEngine::new().run_post(
                r#"
                    pm.sendRequest("http://127.0.0.1:1/nope", function (err, response) {
                        pm.test("error path", function () {
                            pm.expect(err).to.not.be.null;
                            pm.expect(err.message).to.be.a("string");
                            pm.expect(response).to.be.undefined;
                        });
                    });
                "#,
                &res,
                &BTreeMap::new(),
            )
        })
        .await
        .expect("script task");

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.assertions.len(), 1);
        assert!(out.assertions[0].passed, "{:?}", out.assertions[0]);
    }

    #[test]
    fn run_pre_does_not_mutate_input_vars_map() {
        let engine = JsEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");
        let vars = empty_vars();

        let out = engine.run_pre(r#"vars.set("a", "b");"#, &mut req, &vars);

        assert!(
            vars.is_empty(),
            "caller's vars map must not be mutated in place"
        );
        assert_eq!(
            out.var_mutations,
            vec![VarMutation::Set("a".to_string(), "b".to_string())]
        );
    }
}
