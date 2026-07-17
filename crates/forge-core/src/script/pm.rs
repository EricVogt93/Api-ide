//! Postman `pm.*` compatibility shim for the JavaScript engine, so scripts
//! imported from Postman collections run without rewriting: `pm.test`,
//! a mini-chai `pm.expect`, `pm.response` / `pm.request` wrappers and the
//! variable scopes (`pm.environment` / `pm.variables` /
//! `pm.collectionVariables` / `pm.globals` — all backed by Forge's single
//! runtime variable scope).
//!
//! Built as a pure-JS prelude over the host bindings `js.rs` already
//! installs (`vars`, `req`, `res`); the only extra host function is
//! `__pmTestResult`, which records a named test outcome with a failure
//! message.

use std::cell::RefCell;
use std::rc::Rc;

use rquickjs::{Ctx, Function};

use crate::assert::AssertionOutcome;

/// Install `pm` on top of the already-installed `vars` (and, when present,
/// `req`/`res`) bindings. `has_response` gates `pm.response`.
pub(super) fn install_pm(
    ctx: &Ctx<'_>,
    assertions: &Rc<RefCell<Vec<AssertionOutcome>>>,
    has_response: bool,
) -> rquickjs::Result<()> {
    let sink = assertions.clone();
    ctx.globals().set(
        "__pmTestResult",
        Function::new(ctx.clone(), move |name: String, passed: bool, message: String| {
            sink.borrow_mut().push(AssertionOutcome {
                summary: name,
                passed,
                message: if message.is_empty() { None } else { Some(message) },
            });
        })?,
    )?;
    ctx.globals().set("__pmHasResponse", has_response)?;
    ctx.eval::<(), _>(PM_PRELUDE)
}

const PM_PRELUDE: &str = r#"
(function () {
    function show(v) {
        if (typeof v === "string") return JSON.stringify(v);
        try { var s = JSON.stringify(v); return s === undefined ? String(v) : s; }
        catch (e) { return String(v); }
    }
    function deepEqual(a, b) {
        if (a === b) return true;
        if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
        if (Array.isArray(a) !== Array.isArray(b)) return false;
        var ka = Object.keys(a), kb = Object.keys(b);
        if (ka.length !== kb.length) return false;
        for (var i = 0; i < ka.length; i++) {
            if (!Object.prototype.hasOwnProperty.call(b, ka[i])) return false;
            if (!deepEqual(a[ka[i]], b[ka[i]])) return false;
        }
        return true;
    }

    function Expectation(actual, negated) {
        this._actual = actual;
        this._negated = !!negated;
    }
    Expectation.prototype._check = function (pass, msg) {
        var ok = this._negated ? !pass : pass;
        if (!ok) {
            throw new Error("expected " + show(this._actual) + (this._negated ? " not " : " ") + msg);
        }
        return this;
    };
    // Chainable no-op words, chai-style: pm.expect(x).to.be.above(3)
    ["to", "be", "been", "is", "that", "which", "and", "has", "have", "with", "at", "of", "deep", "same"]
        .forEach(function (w) {
            Object.defineProperty(Expectation.prototype, w, { get: function () { return this; } });
        });
    Object.defineProperty(Expectation.prototype, "not", {
        get: function () { return new Expectation(this._actual, !this._negated); }
    });
    // Assertion-on-property-access terminals.
    Object.defineProperty(Expectation.prototype, "ok", {
        get: function () { return this._check(!!this._actual, "to be truthy"); }
    });
    Object.defineProperty(Expectation.prototype, "true", {
        get: function () { return this._check(this._actual === true, "to be true"); }
    });
    Object.defineProperty(Expectation.prototype, "false", {
        get: function () { return this._check(this._actual === false, "to be false"); }
    });
    Object.defineProperty(Expectation.prototype, "null", {
        get: function () { return this._check(this._actual === null, "to be null"); }
    });
    Object.defineProperty(Expectation.prototype, "undefined", {
        get: function () { return this._check(this._actual === undefined, "to be undefined"); }
    });
    Object.defineProperty(Expectation.prototype, "exist", {
        get: function () { return this._check(this._actual !== null && this._actual !== undefined, "to exist"); }
    });
    Object.defineProperty(Expectation.prototype, "empty", {
        get: function () {
            var a = this._actual;
            var len;
            if (typeof a === "string" || Array.isArray(a)) len = a.length;
            else if (a && typeof a === "object") len = Object.keys(a).length;
            else len = NaN;
            return this._check(len === 0, "to be empty");
        }
    });
    Expectation.prototype.equal = function (v) { return this._check(this._actual === v, "to equal " + show(v)); };
    Expectation.prototype.equals = Expectation.prototype.equal;
    Expectation.prototype.eq = Expectation.prototype.equal;
    Expectation.prototype.eql = function (v) { return this._check(deepEqual(this._actual, v), "to deeply equal " + show(v)); };
    Expectation.prototype.eqls = Expectation.prototype.eql;
    Expectation.prototype.a = function (type) {
        var t = Array.isArray(this._actual) ? "array" : (this._actual === null ? "null" : typeof this._actual);
        return this._check(t === String(type), "to be a " + type);
    };
    Expectation.prototype.an = Expectation.prototype.a;
    Expectation.prototype.include = function (v) {
        var a = this._actual;
        var pass;
        if (typeof a === "string") pass = a.indexOf(v) !== -1;
        else if (Array.isArray(a)) pass = a.some(function (x) { return deepEqual(x, v); });
        else if (a && typeof a === "object" && v && typeof v === "object") {
            pass = Object.keys(v).every(function (k) { return deepEqual(a[k], v[k]); });
        } else pass = false;
        return this._check(pass, "to include " + show(v));
    };
    Expectation.prototype.includes = Expectation.prototype.include;
    Expectation.prototype.contain = Expectation.prototype.include;
    Expectation.prototype.contains = Expectation.prototype.include;
    Expectation.prototype.property = function (name, value) {
        var has = this._actual !== null && typeof this._actual === "object" && (name in this._actual);
        if (arguments.length > 1) {
            return this._check(has && deepEqual(this._actual[name], value),
                "to have property " + show(name) + " of " + show(value));
        }
        return this._check(has, "to have property " + show(name));
    };
    Expectation.prototype.lengthOf = function (n) {
        var len = this._actual === null || this._actual === undefined ? NaN
            : (typeof this._actual.length === "number" ? this._actual.length : NaN);
        return this._check(len === n, "to have length " + n + " (was " + len + ")");
    };
    Expectation.prototype.above = function (n) { return this._check(this._actual > n, "to be above " + n); };
    Expectation.prototype.greaterThan = Expectation.prototype.above;
    Expectation.prototype.below = function (n) { return this._check(this._actual < n, "to be below " + n); };
    Expectation.prototype.lessThan = Expectation.prototype.below;
    Expectation.prototype.least = function (n) { return this._check(this._actual >= n, "to be at least " + n); };
    Expectation.prototype.most = function (n) { return this._check(this._actual <= n, "to be at most " + n); };
    Expectation.prototype.within = function (lo, hi) {
        return this._check(this._actual >= lo && this._actual <= hi, "to be within " + lo + ".." + hi);
    };
    Expectation.prototype.match = function (re) {
        var rx = re instanceof RegExp ? re : new RegExp(re);
        return this._check(rx.test(String(this._actual)), "to match " + String(rx));
    };
    Expectation.prototype.matches = Expectation.prototype.match;
    Expectation.prototype.oneOf = function (arr) {
        var self = this;
        var pass = Array.isArray(arr) && arr.some(function (x) { return deepEqual(self._actual, x); });
        return this._check(pass, "to be one of " + show(arr));
    };

    // Every Postman variable scope maps onto Forge's single runtime scope.
    var pmVars = {
        get: function (k) { return vars.get(String(k)); },
        set: function (k, v) { vars.set(String(k), String(v)); },
        has: function (k) { return typeof vars.get(String(k)) !== "undefined"; },
        // ponytail: the host has no variable removal; unset blanks the value.
        unset: function (k) { vars.set(String(k), ""); },
        replaceIn: function (s) {
            return String(s).replace(/\{\{([^}]+)\}\}/g, function (m, name) {
                var v = vars.get(name.trim());
                return typeof v === "undefined" ? m : v;
            });
        }
    };

    var pm = {
        expect: function (v) { return new Expectation(v, false); },
        test: function (name, fn) {
            try {
                fn();
                __pmTestResult(String(name), true, "");
            } catch (e) {
                __pmTestResult(String(name), false, e && e.message ? e.message : String(e));
            }
        },
        environment: pmVars,
        variables: pmVars,
        collectionVariables: pmVars,
        globals: pmVars,
        info: { eventName: __pmHasResponse ? "test" : "prerequest" },
        sendRequest: function () {
            throw new Error("pm.sendRequest is not supported in Forge scripts");
        }
    };

    if (__pmHasResponse) {
        function statusText() { return typeof res.statusText === "string" ? res.statusText : ""; }
        pm.response = {
            get code() { return res.status; },
            get status() { return statusText(); },
            get responseTime() { return res.timeMs; },
            text: function () { return res.bodyText; },
            json: function () {
                var j = res.json();
                if (typeof j === "undefined") throw new Error("response body is not valid JSON");
                return j;
            },
            headers: {
                get: function (n) {
                    var v = res.header(String(n));
                    return typeof v === "undefined" ? null : v;
                },
                has: function (n) { return typeof res.header(String(n)) !== "undefined"; }
            },
            to: {
                have: {
                    status: function (expected) {
                        if (typeof expected === "number") {
                            if (res.status !== expected) {
                                throw new Error("expected response to have status code " + expected + " but got " + res.status);
                            }
                        } else if (statusText().toLowerCase() !== String(expected).toLowerCase()) {
                            throw new Error("expected response status " + show(String(expected)) + " but got " + show(statusText()));
                        }
                    },
                    header: function (name) {
                        if (typeof res.header(String(name)) === "undefined") {
                            throw new Error("expected response to have header " + show(String(name)));
                        }
                    },
                    jsonBody: function () {
                        if (typeof res.json() === "undefined") {
                            throw new Error("expected response to have a JSON body");
                        }
                    }
                },
                be: {
                    get ok() {
                        if (res.status < 200 || res.status >= 300) throw new Error("expected a 2xx response but got " + res.status);
                        return true;
                    },
                    get success() { return this.ok; },
                    get clientError() {
                        if (res.status < 400 || res.status >= 500) throw new Error("expected a 4xx response but got " + res.status);
                        return true;
                    },
                    get serverError() {
                        if (res.status < 500 || res.status >= 600) throw new Error("expected a 5xx response but got " + res.status);
                        return true;
                    }
                }
            }
        };
    } else {
        Object.defineProperty(pm, "response", {
            get: function () { throw new Error("pm.response is only available in post-response scripts"); }
        });
    }

    if (typeof req !== "undefined") {
        pm.request = {
            get url() { return req.url; },
            get method() { return req.method; },
            headers: {
                get: function (n) { return req.getHeader(String(n)); },
                has: function (n) { return typeof req.getHeader(String(n)) !== "undefined"; },
                add: function (h) { req.setHeader(String(h.key), String(h.value)); },
                upsert: function (h) { req.setHeader(String(h.key), String(h.value)); },
                remove: function (n) { req.removeHeader(String(n)); }
            }
        };
    } else {
        Object.defineProperty(pm, "request", {
            get: function () { throw new Error("pm.request is only available in request scripts"); }
        });
    }

    globalThis.pm = pm;
})();
"#;
