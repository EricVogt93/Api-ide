//! Host API surface exposed to scripts: the `req` / `res` / `vars` handle
//! types, `assert`/`test`, `log`, and the small set of stateless helper
//! functions (`uuid`, `timestamp`, `base64_encode`/`base64_decode`).
//!
//! Handle types wrap `Arc<Mutex<..>>` around the real data so that a
//! script's mutations (setting a header, recording a variable) are visible
//! to the caller after the script finishes, even though Rhai only ever
//! hands the script owned clones of the `Dynamic` values it works with.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rhai::{Dynamic, Engine};

use crate::assert::AssertionOutcome;
use crate::exec::{ExecutionResult, ResolvedBody, ResolvedRequest};

use super::VarMutation;

/// Lock a mutex, recovering from poisoning instead of panicking. These
/// mutexes only ever guard plain data behind a single-threaded script
/// execution, so poisoning should never happen in practice; recovering
/// keeps script execution panic-free even if it somehow did.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Register the small set of helper functions that don't need any
/// per-execution state. Registered once, at [`super::ScriptEngine`]
/// construction time.
pub(crate) fn register_stateless(engine: &mut Engine) {
    engine.register_fn("uuid", || uuid::Uuid::new_v4().to_string());
    engine.register_fn("timestamp", || chrono::Utc::now().timestamp());
    engine.register_fn("base64_encode", |s: String| BASE64.encode(s.as_bytes()));
    engine.register_fn("base64_decode", |s: String| -> String {
        match BASE64.decode(s.as_bytes()) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => String::new(),
        }
    });
}

/// Install `log(msg)` and the `print`/`debug` hooks, both feeding the same
/// shared log buffer, in execution order.
pub(crate) fn register_log(engine: &mut Engine, log: Arc<Mutex<Vec<String>>>) {
    let for_fn = log.clone();
    engine.register_fn("log", move |msg: Dynamic| {
        lock(&for_fn).push(msg.to_string());
    });

    let for_print = log;
    engine.on_print(move |s: &str| {
        lock(&for_print).push(s.to_string());
    });
}

/// Handle to the [`ResolvedRequest`] being scripted by a pre-request hook.
/// Cheap to clone (an `Arc` bump); every clone mutates the same underlying
/// request.
#[derive(Clone)]
pub(crate) struct ReqHandle(Arc<Mutex<ResolvedRequest>>);

impl ReqHandle {
    pub(crate) fn new(inner: Arc<Mutex<ResolvedRequest>>) -> Self {
        Self(inner)
    }

    fn get_url(&mut self) -> String {
        lock(&self.0).url.clone()
    }

    fn set_url(&mut self, value: String) {
        lock(&self.0).url = value;
    }

    fn get_method(&mut self) -> String {
        lock(&self.0).method.as_str().to_string()
    }

    fn set_header(&mut self, name: String, value: String) {
        let mut req = lock(&self.0);
        match req
            .headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        {
            Some(existing) => existing.1 = value,
            None => req.headers.push((name, value)),
        }
    }

    fn get_header(&mut self, name: String) -> Dynamic {
        let req = lock(&self.0);
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
            .map(|(_, v)| Dynamic::from(v.clone()))
            .unwrap_or(Dynamic::UNIT)
    }

    fn remove_header(&mut self, name: String) {
        lock(&self.0)
            .headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
    }

    fn set_body_text(&mut self, text: String) {
        let mut req = lock(&self.0);
        let content_type = match &req.body {
            ResolvedBody::Bytes { content_type, .. } => content_type.clone(),
            _ => None,
        };
        req.body = ResolvedBody::Bytes {
            content_type,
            data: text.into_bytes(),
        };
    }

    fn get_body_text(&mut self) -> String {
        match &lock(&self.0).body {
            ResolvedBody::Bytes { data, .. } => String::from_utf8_lossy(data).into_owned(),
            _ => String::new(),
        }
    }
}

/// Register the `Request` type (bound to the script variable `req`) with
/// its property getters/setters and methods.
pub(crate) fn register_req_type(engine: &mut Engine) {
    engine
        .register_type_with_name::<ReqHandle>("Request")
        .register_get_set("url", ReqHandle::get_url, ReqHandle::set_url)
        .register_get("method", ReqHandle::get_method)
        .register_fn("set_header", ReqHandle::set_header)
        .register_fn("get_header", ReqHandle::get_header)
        .register_fn("remove_header", ReqHandle::remove_header)
        .register_fn("set_body_text", ReqHandle::set_body_text)
        .register_get("body_text", ReqHandle::get_body_text);
}

/// Read-only handle to the [`ExecutionResult`] scripted by a post-response
/// hook.
#[derive(Clone)]
pub(crate) struct ResHandle(Arc<ExecutionResult>);

impl ResHandle {
    pub(crate) fn new(inner: Arc<ExecutionResult>) -> Self {
        Self(inner)
    }

    fn get_status(&mut self) -> i64 {
        i64::from(self.0.status)
    }

    fn get_body_text(&mut self) -> String {
        self.0.text().into_owned()
    }

    fn get_time_ms(&mut self) -> i64 {
        // `Duration::as_millis` returns u128; response times never come
        // close to overflowing an i64 worth of milliseconds.
        self.0.timing.total.as_millis() as i64
    }

    fn header(&mut self, name: String) -> Dynamic {
        self.0
            .header(&name)
            .map(|v| Dynamic::from(v.to_string()))
            .unwrap_or(Dynamic::UNIT)
    }

    fn json(&mut self) -> Dynamic {
        self.0
            .json()
            .and_then(|value| rhai::serde::to_dynamic(value).ok())
            .unwrap_or(Dynamic::UNIT)
    }
}

/// Register the `Response` type (bound to the script variable `res`).
pub(crate) fn register_res_type(engine: &mut Engine) {
    engine
        .register_type_with_name::<ResHandle>("Response")
        .register_get("status", ResHandle::get_status)
        .register_get("body_text", ResHandle::get_body_text)
        .register_get("time_ms", ResHandle::get_time_ms)
        .register_fn("header", ResHandle::header)
        .register_fn("json", ResHandle::json);
}

/// Handle to the runtime variable scope (bound to the script variable
/// `vars`). Reads see values set earlier in the same script; every
/// `set` call is also recorded in order so the caller can persist it into
/// the real runtime scope.
#[derive(Clone)]
pub(crate) struct VarsHandle {
    values: Arc<Mutex<BTreeMap<String, String>>>,
    mutations: Arc<Mutex<Vec<VarMutation>>>,
}

impl VarsHandle {
    pub(crate) fn new(
        values: Arc<Mutex<BTreeMap<String, String>>>,
        mutations: Arc<Mutex<Vec<VarMutation>>>,
    ) -> Self {
        Self { values, mutations }
    }

    fn get(&mut self, name: String) -> Dynamic {
        lock(&self.values)
            .get(&name)
            .map(|v| Dynamic::from(v.clone()))
            .unwrap_or(Dynamic::UNIT)
    }

    fn set(&mut self, name: String, value: String) {
        lock(&self.values).insert(name.clone(), value.clone());
        lock(&self.mutations).push(VarMutation::Set(name, value));
    }

    fn unset(&mut self, name: String) {
        lock(&self.values).remove(&name);
        lock(&self.mutations).push(VarMutation::Unset(name));
    }
}

/// Register the `Vars` type (bound to the script variable `vars`).
pub(crate) fn register_vars_type(engine: &mut Engine) {
    engine
        .register_type_with_name::<VarsHandle>("Vars")
        .register_fn("get", VarsHandle::get)
        .register_fn("set", VarsHandle::set)
        .register_fn("unset", VarsHandle::unset);
}

/// Install the post-response `assert(cond, message)` and
/// `test(name, cond)` host functions. Neither aborts the script on
/// failure; they simply record an [`AssertionOutcome`].
pub(crate) fn register_assertions(
    engine: &mut Engine,
    assertions: Arc<Mutex<Vec<AssertionOutcome>>>,
) {
    let for_assert = assertions.clone();
    engine.register_fn("assert", move |cond: bool, message: String| {
        lock(&for_assert).push(AssertionOutcome {
            summary: message,
            passed: cond,
            message: None,
        });
    });

    let for_test = assertions;
    engine.register_fn("test", move |name: String, cond: bool| {
        lock(&for_test).push(AssertionOutcome {
            summary: name,
            passed: cond,
            message: None,
        });
    });
}
