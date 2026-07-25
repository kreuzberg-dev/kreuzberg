//! NER (Named Entity Recognition) bridge with injected dispatch.
//!
//! The WASM engine calls an externally-injected JavaScript object that
//! implements a `ner(text, categories)` async method, called positionally to
//! match [`call_injected_ner`]. The host returns a promise resolving to an
//! array of entities (`{ category, text, start, end, confidence? }`).
//!
//! The *injected* path only; with nothing injected the engine reports NER as
//! unavailable. To run a model inside the binary instead see
//! [`crate::bridge::ner_model::NerModel`], a separate entry point that does not
//! route through this bridge: local inference is synchronous, so the timeout here
//! could not interrupt it.

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::prelude::*;

use xberg::types::entity::{Entity, EntityCategory};

use crate::bridge::js_from_any;

/// Read the `categories` option off a JS options bag.
///
/// Missing, `null`, `undefined`, a non-array, or a non-string element degrade to
/// an empty list, which backends read as "use the defaults", not "detect
/// nothing". Unknown names become [`EntityCategory::Custom`] zero-shot labels.
///
/// Shared by [`crate::engine::XbergEngine::ner`] and
/// [`crate::bridge::ner_model::NerModel::detect`] so they cannot drift.
pub(crate) fn categories_from_opts(opts: &JsValue) -> Vec<EntityCategory> {
    if opts.is_undefined() || opts.is_null() {
        return Vec::new();
    }
    Reflect::get(opts, &JsValue::from_str("categories"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_string())
                .map(EntityCategory::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve NER through the injected backend, with a configurable bridge timeout.
pub async fn resolve_ner_with_timeout(
    injected: Option<js_sys::Object>,
    text: &str,
    categories: &[EntityCategory],
    timeout_ms: u32,
) -> Result<Vec<Entity>, JsValue> {
    match injected {
        Some(obj) => call_injected_ner(obj, text, categories, timeout_ms).await,
        None => Err(js_from_any(
            "NER unavailable: no NER backend injected. Pass a `ner` object in the engine injection, \
             or run a model in the browser with NerModel.load({ weights, tokenizer, encoderConfig })",
        )),
    }
}

/// The wire form of an [`EntityCategory`]: the serde snake_case name for the
/// built-in variants, the raw label for `Custom`. `serde_json::to_value`
/// alone would render `Custom("x")` as an object and lose the label.
fn category_wire_name(category: &EntityCategory) -> String {
    match category {
        EntityCategory::Custom(label) => label.clone(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
    }
}

/// Call the injected JS `ner(text, categories)` method and deserialize the
/// returned promise into a `Vec<Entity>`.
async fn call_injected_ner(
    obj: Object,
    text: &str,
    categories: &[EntityCategory],
    timeout_ms: u32,
) -> Result<Vec<Entity>, JsValue> {
    let fn_val = Reflect::get(&obj, &JsValue::from_str("ner"))
        .map_err(|e| js_from_any(format!("failed to read 'ner' property: {e:?}")))?;
    let func: Function = fn_val
        .dyn_into()
        .map_err(|_| js_from_any("injected NER object has no 'ner' function"))?;

    let js_text = JsValue::from_str(text);
    let js_cats = js_sys::Array::new();
    for c in categories {
        js_cats.push(&JsValue::from_str(&category_wire_name(c)));
    }
    let args = js_sys::Array::of2(&js_text, &js_cats);

    let result = func.apply(&obj, &args)?;
    let promise = Promise::resolve(&result);
    let js_val = crate::bridge::timed_js_future_with_timeout(promise, timeout_ms).await?;

    serde_wasm_bindgen::from_value(js_val).map_err(|e| js_from_any(format!("failed to deserialize NER result: {e}")))
}
