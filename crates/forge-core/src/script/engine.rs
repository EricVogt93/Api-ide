use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, Scope};

use crate::assert::AssertionOutcome;
use crate::exec::{ExecutionResult, ResolvedRequest};

use super::api::{
    self, register_assertions, register_log, register_req_type, register_res_type,
    register_vars_type, ReqHandle, ResHandle, VarsHandle,
};

/// One ordered mutation to the runtime variable scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarMutation {
    Set(String, String),
    Unset(String),
}

/// Result of running one pre-request or post-response script.
#[derive(Debug, Clone, Default)]
pub struct ScriptOutput {
    /// Lines captured from `log(..)` and the built-in `print`/`debug`.
    pub log: Vec<String>,
    /// Compile or runtime failure, if the script errored. `None` means the
    /// script ran to completion (individual `assert`/`test` failures are
    /// not script errors — see [`ScriptOutput::assertions`]).
    pub error: Option<String>,
    /// Assertions recorded by `assert(cond, message)` / `test(name, cond)`
    /// during a post-response script. Always empty for pre-request scripts.
    pub assertions: Vec<AssertionOutcome>,
    /// Ordered `vars.set` / `vars.unset` calls for the caller to persist
    /// into the runtime variable scope.
    pub var_mutations: Vec<VarMutation>,
}

/// A reusable, sandboxed Rhai engine for pre-request / post-response
/// scripting hooks.
///
/// Sandbox limits and the disabled `eval` symbol are configured once, at
/// construction time. Everything execution-specific (the `req`/`res`/`vars`
/// bindings, `assert`/`test` recording, log capture) is installed fresh for
/// each call to [`Self::run_pre`] / [`Self::run_post`], so concurrent runs
/// never see each other's state — only one script runs at a time per
/// `ScriptEngine` instance (guarded by an internal mutex); use multiple
/// instances, e.g. one per worker, to run scripts in parallel.
pub struct ScriptEngine {
    engine: Mutex<Engine>,
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptEngine {
    /// Build a new engine with sandbox limits applied and `eval` disabled.
    pub fn new() -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(1_000_000);
        engine.set_max_call_levels(32);
        engine.set_max_string_size(1_000_000);
        engine.set_max_array_size(10_000);
        engine.set_max_map_size(10_000);
        engine.disable_symbol("eval");
        api::register_stateless(&mut engine);
        Self {
            engine: Mutex::new(engine),
        }
    }

    /// Run a pre-request script. `req` is mutated in place through the
    /// script's `req.*` calls; `vars` is the read/write variable scope
    /// visible to the script as `vars.*`.
    pub fn run_pre(
        &self,
        script: &str,
        req: &mut ResolvedRequest,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        let mut engine = self.lock_engine();

        let req_state = Arc::new(Mutex::new(req.clone()));
        let vars_state = Arc::new(Mutex::new(vars.clone()));
        let var_mutations = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));

        register_req_type(&mut engine);
        register_vars_type(&mut engine);
        register_log(&mut engine, log.clone());

        let mut scope = Scope::new();
        scope.push("req", ReqHandle::new(req_state.clone()));
        scope.push("vars", VarsHandle::new(vars_state, var_mutations.clone()));

        let (log, error) = run_script(&engine, script, &mut scope, &log);

        if let Ok(updated) = req_state.lock() {
            *req = updated.clone();
        }

        ScriptOutput {
            log,
            error,
            assertions: Vec::new(),
            var_mutations: var_mutations.lock().map(|v| v.clone()).unwrap_or_default(),
        }
    }

    /// Run a post-response script. `res` is read-only from the script's
    /// point of view; `vars` behaves as in [`Self::run_pre`]. `assert` and
    /// `test` calls in the script are recorded into
    /// [`ScriptOutput::assertions`] and never abort the script.
    pub fn run_post(
        &self,
        script: &str,
        res: &ExecutionResult,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        let mut engine = self.lock_engine();

        let vars_state = Arc::new(Mutex::new(vars.clone()));
        let var_mutations = Arc::new(Mutex::new(Vec::new()));
        let assertions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));

        register_res_type(&mut engine);
        register_vars_type(&mut engine);
        register_log(&mut engine, log.clone());
        register_assertions(&mut engine, assertions.clone());

        let mut scope = Scope::new();
        scope.push("res", ResHandle::new(Arc::new(res.clone())));
        scope.push("vars", VarsHandle::new(vars_state, var_mutations.clone()));

        let (log, error) = run_script(&engine, script, &mut scope, &log);

        ScriptOutput {
            log,
            error,
            assertions: assertions.lock().map(|v| v.clone()).unwrap_or_default(),
            var_mutations: var_mutations.lock().map(|v| v.clone()).unwrap_or_default(),
        }
    }

    /// Run a suite lifecycle hook (`beforeAll`/`beforeEach`/`afterEach`/
    /// `afterAll`). Exposes `vars`/`log`/`assert`/`test`/helpers, same as
    /// [`Self::run_post`], but never binds `req` or `res` — hooks that need
    /// the response (`afterEach`/`afterAll`) go through [`Self::run_post`]
    /// instead, reusing its `res` plumbing.
    pub fn run_hook(&self, script: &str, vars: &BTreeMap<String, String>) -> ScriptOutput {
        let mut engine = self.lock_engine();

        let vars_state = Arc::new(Mutex::new(vars.clone()));
        let var_mutations = Arc::new(Mutex::new(Vec::new()));
        let assertions = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));

        register_vars_type(&mut engine);
        register_log(&mut engine, log.clone());
        register_assertions(&mut engine, assertions.clone());

        let mut scope = Scope::new();
        scope.push("vars", VarsHandle::new(vars_state, var_mutations.clone()));

        let (log, error) = run_script(&engine, script, &mut scope, &log);

        ScriptOutput {
            log,
            error,
            assertions: assertions.lock().map(|v| v.clone()).unwrap_or_default(),
            var_mutations: var_mutations.lock().map(|v| v.clone()).unwrap_or_default(),
        }
    }

    fn lock_engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Compile and run `script` against `scope`, never panicking. Returns the
/// captured log lines and, if the script failed to compile or errored at
/// runtime (including hitting a sandbox limit), a human-readable error
/// message with source position.
fn run_script(
    engine: &Engine,
    script: &str,
    scope: &mut Scope,
    log: &Arc<Mutex<Vec<String>>>,
) -> (Vec<String>, Option<String>) {
    let ast = match engine.compile(script) {
        Ok(ast) => ast,
        Err(err) => return (Vec::new(), Some(format!("script compile error: {err}"))),
    };

    let result = engine.eval_ast_with_scope::<Dynamic>(scope, &ast);
    let log = log.lock().map(|v| v.clone()).unwrap_or_default();

    match result {
        Ok(_) => (log, None),
        Err(err) => (log, Some(format!("script error: {err}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{Hop, ResolvedBody, Sizes, TimingBreakdown};
    use crate::model::Method;
    use std::time::Duration;

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
                ttfb: Duration::from_millis(12),
                download: Duration::from_millis(3),
                total: Duration::from_millis(42),
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
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test/old");
        req.headers.push(("X-Existing".into(), "1".into()));

        let script = r#"
            req.url = req.url + "?patched=1";
            req.set_header("X-New", "hello");
            req.set_header("X-Existing", "2");
            req.remove_header("X-Existing");
            req.set_body_text("payload");
            vars.set("token", "abc123");
            log("done");
        "#;

        let out = engine.run_pre(script, &mut req, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(req.url, "https://example.test/old?patched=1");
        assert!(req.header("X-New").is_some());
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
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");
        let mut vars = BTreeMap::new();
        vars.insert("base".to_string(), "https://api.test".to_string());

        let out = engine.run_pre(r#"req.url = vars.get("base") + "/path";"#, &mut req, &vars);

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(req.url, "https://api.test/path");
    }

    #[test]
    fn variable_mutations_preserve_set_and_unset_order() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");
        let mut vars = BTreeMap::new();
        vars.insert("token".to_string(), "old".to_string());

        let out = engine.run_pre(
            r#"vars.unset("token"); vars.set("token", "new");"#,
            &mut req,
            &vars,
        );

        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(
            out.var_mutations,
            vec![
                VarMutation::Unset("token".to_string()),
                VarMutation::Set("token".to_string(), "new".to_string())
            ]
        );
    }

    #[test]
    fn pre_script_missing_var_is_unit() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                let v = vars.get("missing");
                if v == () { log("was-unit"); } else { log("was-set"); }
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["was-unit".to_string()]);
    }

    #[test]
    fn post_script_records_pass_and_fail_assertions() {
        let engine = ScriptEngine::new();
        let res = sample_result(200, r#"{"ok":true}"#);

        let script = r#"
            assert(res.status == 200, "status is 200");
            assert(res.status == 404, "status is 404");
            test("has ok field", res.json().ok == true);
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
        let engine = ScriptEngine::new();
        let res = sample_result(201, r#"{"name":"forge","count":3}"#);

        let script = r#"
            let j = res.json();
            log(j.name);
            log(j.count.to_string());
            log(res.body_text);
            log(res.header("Content-Type"));
            log(res.time_ms.to_string());
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
    fn post_script_vars_set_is_captured() {
        let engine = ScriptEngine::new();
        let res = sample_result(200, r#"{"id":"xyz"}"#);

        let out = engine.run_post(r#"vars.set("id", res.json().id);"#, &res, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.var_mutations,
            vec![VarMutation::Set("id".to_string(), "xyz".to_string())]
        );
    }

    #[test]
    fn log_capture_preserves_order_across_print_and_log() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                log("one");
                print("two");
                log("three");
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(
            out.log,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    #[test]
    fn infinite_loop_is_terminated_by_operation_limit() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("loop {}", &mut req, &empty_vars());

        let err = out.error.expect("expected an operation-limit error");
        assert!(
            err.to_lowercase().contains("too many operations"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn compile_error_is_reported_with_line_info() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("let x = ;", &mut req, &empty_vars());

        let err = out.error.expect("expected a compile error");
        assert!(
            err.starts_with("script compile error:"),
            "unexpected error: {err}"
        );
        assert!(err.contains("line 1"), "expected line info in: {err}");
    }

    #[test]
    fn runtime_error_is_reported_with_line_info() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("\n\nlet x = undefined_fn();", &mut req, &empty_vars());

        let err = out.error.expect("expected a runtime error");
        assert!(err.starts_with("script error:"), "unexpected error: {err}");
        assert!(err.contains("line 3"), "expected line info in: {err}");
    }

    #[test]
    fn eval_is_disabled() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(r#"eval("1 + 1")"#, &mut req, &empty_vars());

        assert!(out.error.is_some(), "expected eval to be rejected");
    }

    #[test]
    fn base64_roundtrip_helpers() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                let encoded = base64_encode("hello world");
                log(encoded);
                log(base64_decode(encoded));
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
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre("log(uuid());", &mut req, &empty_vars());

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log.len(), 1);
        let id = &out.log[0];
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
        assert!(uuid::Uuid::parse_str(id).is_ok(), "not a valid uuid: {id}");
    }

    #[test]
    fn timestamp_returns_current_epoch_seconds() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let before = chrono::Utc::now().timestamp();
        let out = engine.run_pre("log(timestamp().to_string());", &mut req, &empty_vars());
        let after = chrono::Utc::now().timestamp();

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        let ts: i64 = out.log[0].parse().expect("timestamp should be an integer");
        assert!(ts >= before - 1 && ts <= after + 1);
    }

    #[test]
    fn host_environment_access_is_rejected() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(r#"env("PATH");"#, &mut req, &empty_vars());

        assert!(
            out.error
                .as_deref()
                .is_some_and(|error| error.contains("Function not found")),
            "{:?}",
            out.error
        );
    }

    #[test]
    fn get_header_returns_unit_when_missing() {
        let engine = ScriptEngine::new();
        let mut req = ResolvedRequest::new(Method::Get, "https://example.test");

        let out = engine.run_pre(
            r#"
                let v = req.get_header("X-Missing");
                if v == () { log("unit"); } else { log(v); }
            "#,
            &mut req,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["unit".to_string()]);
    }

    #[test]
    fn res_json_is_unit_for_non_json_body() {
        let engine = ScriptEngine::new();
        let res = sample_result(200, "not json");

        let out = engine.run_post(
            r#"
                let v = res.json();
                if v == () { log("unit"); } else { log("not-unit"); }
            "#,
            &res,
            &empty_vars(),
        );

        assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
        assert_eq!(out.log, vec!["unit".to_string()]);
    }

    #[test]
    fn hook_script_sets_vars_and_records_assertions_without_req_or_res() {
        let engine = ScriptEngine::new();

        let out = engine.run_hook(
            r#"
                vars.set("suite", "started");
                assert(vars.get("suite") == "started", "suite var visible");
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
    fn hook_script_has_no_req_or_res_binding() {
        let engine = ScriptEngine::new();

        let out = engine.run_hook("req.url;", &empty_vars());
        assert!(
            out.error.is_some(),
            "expected `req` to be unavailable in a hook script"
        );

        let out = engine.run_hook("res.status;", &empty_vars());
        assert!(
            out.error.is_some(),
            "expected `res` to be unavailable in a hook script"
        );
    }

    #[test]
    fn run_pre_does_not_mutate_input_vars_map() {
        let engine = ScriptEngine::new();
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
