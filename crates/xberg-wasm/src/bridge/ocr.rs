//! OCR (Optical Character Recognition) bridge with injected dispatch.
//!
//! The WASM engine calls an externally-injected JavaScript object that
//! implements an `ocr(imageBytes, options)` async method. The host returns
//! a promise resolving to `{ text: string, lines: Array<{ text: string,
//! confidence: number, bbox?: { x, y, width, height } }> }`; real per-line
//! geometry, not just a flat string. `lines` is optional on the wire; a
//! missing or malformed `lines` array degrades to an empty vec rather than
//! an error, since `text` alone is still useful.
//!
//! There is no in-binary fallback: when no backend is injected, OCR is
//! unavailable and calls return an error saying so.

use js_sys::{Function, Object, Promise, Reflect};

use crate::bridge::js_from_any;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// A single OCR-detected line's bounding box, in source-image pixel space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBbox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A single OCR-detected line of text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLineResult {
    pub text: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<OcrBbox>,
}

/// Full OCR result for one image: the concatenated text plus per-line
/// geometry, when the backend provides it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    #[serde(default)] // missing `lines` degrades to empty vec, not an error ~keep
    pub lines: Vec<OcrLineResult>,
}

/// Resolve OCR through the injected backend and return extracted text with
/// per-line geometry, with a configurable bridge timeout.
///
/// Returns an error when `injected` is `None` or `image_bytes` is empty.
pub async fn resolve_ocr_with_timeout(
    injected: Option<js_sys::Object>,
    image_bytes: &[u8],
    language: &str,
    timeout_ms: u32,
) -> Result<OcrResult, JsValue> {
    if image_bytes.is_empty() {
        return Err(js_from_any("OCR input image is empty"));
    }
    match injected {
        Some(obj) => call_injected_ocr(obj, image_bytes, language, timeout_ms).await,
        None => Err(js_from_any(
            "OCR unavailable: no OCR backend injected; pass an `ocr` object in the engine injection",
        )),
    }
}

/// Call the injected JS `ocr(imageBytes, { language })` method.
async fn call_injected_ocr(
    obj: Object,
    image_bytes: &[u8],
    language: &str,
    timeout_ms: u32,
) -> Result<OcrResult, JsValue> {
    let fn_val = Reflect::get(&obj, &JsValue::from_str("ocr"))
        .map_err(|e| js_from_any(format!("failed to read 'ocr' property: {e:?}")))?;
    let func: Function = fn_val
        .dyn_into()
        .map_err(|_| js_from_any("injected OCR object has no 'ocr' function"))?;

    let js_bytes = js_sys::Uint8Array::from(image_bytes);
    let opts = js_sys::Object::new();
    Reflect::set(&opts, &JsValue::from_str("language"), &JsValue::from_str(language))?;

    let args = js_sys::Array::of2(&js_bytes, &opts);
    let result = func.apply(&obj, &args)?;
    let promise = Promise::resolve(&result);
    let js_val = crate::bridge::timed_js_future_with_timeout(promise, timeout_ms).await?;

    serde_wasm_bindgen::from_value(js_val).map_err(|e| js_from_any(format!("failed to deserialize ocr result: {e}")))
}
