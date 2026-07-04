//! Sandboxed Rhai scripting: pre-request and post-response hooks with a
//! `req` / `res` / `vars` / `assert` host API.
//!
//! A single [`ScriptEngine`] is built once — sandbox limits (operation
//! count, call depth, string/array/map sizes) are applied and the
//! built-in `eval` function is disabled at construction time — and then
//! reused for every script execution. Each call to [`ScriptEngine::run_pre`]
//! or [`ScriptEngine::run_post`] gets its own isolated scope and host
//! state, so scripts can never see state left over from another request.
//!
//! Scripts never abort the caller: compile errors, runtime errors and
//! runaway scripts (operation-limit exceeded) are all captured into
//! [`ScriptOutput::error`] instead of panicking or propagating.

mod api;
mod engine;
mod js;

use std::collections::BTreeMap;

pub use engine::{ScriptEngine, ScriptOutput};
pub use js::JsEngine;

use crate::exec::{ExecutionResult, ResolvedRequest};
use crate::model::ScriptLang;

/// Dispatches pre-request/post-response/hook scripting to whichever engine
/// (`Rhai` or `JavaScript`) a request or suite hook is configured to use.
/// Owns one of each engine so both are ready without per-call setup cost.
#[derive(Default)]
pub struct Scripting {
    rhai: ScriptEngine,
    js: JsEngine,
}

impl Scripting {
    pub fn new() -> Self {
        Self { rhai: ScriptEngine::new(), js: JsEngine::new() }
    }

    /// Run a pre-request script in the given language. See
    /// [`ScriptEngine::run_pre`] / [`JsEngine::run_pre`].
    pub fn run_pre(
        &self,
        lang: ScriptLang,
        script: &str,
        req: &mut ResolvedRequest,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        match lang {
            ScriptLang::Rhai => self.rhai.run_pre(script, req, vars),
            ScriptLang::Js => self.js.run_pre(script, req, vars),
        }
    }

    /// Run a post-response script (or an `afterEach`/`afterAll` suite hook)
    /// in the given language. See [`ScriptEngine::run_post`] /
    /// [`JsEngine::run_post`].
    pub fn run_post(
        &self,
        lang: ScriptLang,
        script: &str,
        res: &ExecutionResult,
        vars: &BTreeMap<String, String>,
    ) -> ScriptOutput {
        match lang {
            ScriptLang::Rhai => self.rhai.run_post(script, res, vars),
            ScriptLang::Js => self.js.run_post(script, res, vars),
        }
    }

    /// Run a `beforeAll`/`beforeEach` suite hook (no `req`/`res` in scope)
    /// in the given language. See [`ScriptEngine::run_hook`] /
    /// [`JsEngine::run_hook`].
    pub fn run_hook(&self, lang: ScriptLang, script: &str, vars: &BTreeMap<String, String>) -> ScriptOutput {
        match lang {
            ScriptLang::Rhai => self.rhai.run_hook(script, vars),
            ScriptLang::Js => self.js.run_hook(script, vars),
        }
    }
}
