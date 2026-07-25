//! JS-facing backend plumbing.
//!
//! Two ways a capability gets satisfied here: an *injected* JS backend the host
//! supplies ([`ner`], [`ocr`]), or an *in-binary* model the host feeds bytes to
//! ([`ner_model`]). Both report failures through [`js_from_any`]. The timeout
//! race below serves only the injected paths: in-binary inference is synchronous,
//! so a `Promise.race` could not interrupt it even if one were armed.
//!
//! Hand-written module (declared via `custom_rust_modules` in `alef.toml`),
//! not managed by alef.

pub mod ner;
pub mod ner_model;
pub mod ocr;

use std::sync::OnceLock;

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn warn(msg: &str);
}

pub(crate) const BRIDGE_TIMEOUT_MS: u32 = 30_000;

/// Convert a Display error into a JsValue suitable for propagation.
pub(crate) fn js_from_any(v: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&v.to_string())
}

/// Build (and cache) the `Promise.race` timeout helper via `Function::new_with_args`.
///
/// # CSP constraint
///
/// `Function::new_with_args` compiles a function body via the JS `Function`
/// constructor, which is equivalent to `eval` and is blocked by a
/// `Content-Security-Policy` without `'unsafe-eval'` (or a `script-src`
/// exempting `Function`/`eval`). Hosts embedding this crate under a strict
/// CSP must relax it to allow `'unsafe-eval'`, or the first bridge call
/// throws a CSP violation instead of racing the timeout. There is currently
/// no CSP-safe fallback; the timeout wrapper is skipped only when arming the
/// race itself fails (e.g. engine-level OOM), not when a CSP blocks the
/// `Function` constructor at page-load or first-call time.
fn get_timeout_racer() -> &'static js_sys::Function {
    static RACER: OnceLock<js_sys::Function> = OnceLock::new();
    RACER.get_or_init(|| {
        js_sys::Function::new_with_args(
            "p, ms",
            "return (() => {\n\
             let id;\n\
             const timer = new Promise((_, reject) => {\n\
                 id = setTimeout(() => reject(new Error('bridge call timed out')), ms);\n\
             });\n\
             const raced = Promise.race([p, timer]);\n\
             raced.then(() => clearTimeout(id), () => clearTimeout(id));\n\
             return raced;\n\
             })()",
        )
    })
}

/// Wrap a JS `Promise` with a timeout.
///
/// If the promise does not settle within `ms` milliseconds, the returned
/// promise rejects with an `Error("bridge call timed out")`.  Uses
/// `Promise.race` under the hood so the original promise is still cancellable.
/// The timer is cleared once the race settles; the cleanup chain hangs off the
/// raced promise with both handlers attached, so it can never itself become an
/// unhandled rejection when the bridge call fails.
pub fn with_timeout(promise: js_sys::Promise, ms: u32) -> js_sys::Promise {
    let racer = get_timeout_racer();
    let p_val = JsValue::from(&promise);
    match racer.call2(&JsValue::NULL, &p_val, &JsValue::from(ms)) {
        Ok(val) => val.into(),
        Err(_) => {
            // Arming the race can only fail on engine-level errors (e.g. OOM).
            // Degrade to the untimed promise, but say so instead of silently
            // dropping the timeout. ~keep
            warn("xberg-wasm: failed to arm bridge timeout; proceeding without one");
            promise
        }
    }
}

/// Convenience wrapper: create a timed-out `JsFuture` from a `Promise`.
pub fn timed_js_future(promise: js_sys::Promise) -> wasm_bindgen_futures::JsFuture {
    wasm_bindgen_futures::JsFuture::from(with_timeout(promise, BRIDGE_TIMEOUT_MS))
}

/// Convenience wrapper: create a `JsFuture` with a custom timeout from a `Promise`.
pub fn timed_js_future_with_timeout(promise: js_sys::Promise, ms: u32) -> wasm_bindgen_futures::JsFuture {
    wasm_bindgen_futures::JsFuture::from(with_timeout(promise, ms))
}
