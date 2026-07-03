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

pub use engine::{ScriptEngine, ScriptOutput};
