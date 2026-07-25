//! In-binary NER: a GLiNER2 model resident in WASM linear memory.
//!
//! Counterpart to [`crate::bridge::ner`], which calls out to injected JS. Here
//! inference runs inside the binary via the pure-Rust candle GLiNER2 backend.
//!
//! Exported as a class, not a free function like `detectLayout`: the weights run
//! to hundreds of MB, so the model is loaded once and stays resident. wasm-bindgen's
//! generated `free()` is optional (a FinalizationRegistry reclaims it), but worth
//! calling at this size — GC timing is unobservable and linear memory never shrinks.
//!
//! Inference is synchronous and wasm32 is single-threaded, so `detect` holds the
//! thread until it returns; drive it from a Web Worker to avoid main-thread jank.
//! The methods are `async` to match the rest of the JS surface, not because they
//! yield.

use wasm_bindgen::prelude::*;

use xberg::text::ner::NerBackend;
use xberg::text::ner::candle::CandleBackend;

use crate::bridge::js_from_any;
use crate::bridge::ner::categories_from_opts;

/// Read a required byte-buffer field off a JS options object.
///
/// Accepts `Uint8Array` or `ArrayBuffer`. Rejects by field name: the three
/// buffers share a type, so a transposition would otherwise surface later as an
/// opaque parse failure.
fn required_bytes(obj: &JsValue, field: &str) -> Result<Vec<u8>, JsValue> {
    let value = js_sys::Reflect::get(obj, &JsValue::from_str(field))
        .map_err(|_| js_from_any(format!("failed to read '{field}'")))?;

    if value.is_undefined() || value.is_null() {
        return Err(js_from_any(format!(
            "missing '{field}': expected a Uint8Array or ArrayBuffer of model bytes"
        )));
    }

    let bytes = if value.is_instance_of::<js_sys::Uint8Array>() {
        value.unchecked_into::<js_sys::Uint8Array>().to_vec()
    } else if value.is_instance_of::<js_sys::ArrayBuffer>() {
        js_sys::Uint8Array::new(&value).to_vec()
    } else {
        return Err(js_from_any(format!(
            "'{field}' must be a Uint8Array or ArrayBuffer"
        )));
    };

    if bytes.is_empty() {
        return Err(js_from_any(format!("'{field}' is empty")));
    }

    Ok(bytes)
}

/// A GLiNER2 model loaded into WASM memory, detecting entities locally.
///
/// Construct with [`NerModel::load`], call [`NerModel::detect`] as many times as
/// needed, and call `free()` from JS when finished to release the weights.
///
/// ```js
/// const model = await NerModel.load({ weights, tokenizer, encoderConfig });
/// const entities = await model.detect("Alice works at Acme Corp", {
///   categories: ["person", "organization"],
/// });
/// model.free();
/// ```
#[wasm_bindgen(js_name = "NerModel")]
pub struct NerModel {
    backend: CandleBackend,
}

#[wasm_bindgen(js_class = "NerModel")]
impl NerModel {
    /// Load a GLiNER2 model from bytes the host has already fetched.
    ///
    /// `options` takes three byte buffers, each a `Uint8Array` or `ArrayBuffer`:
    /// `weights` (`model.safetensors`), `tokenizer` (`tokenizer.json`), and
    /// `encoderConfig` (the encoder `config.json`). Named rather than positional
    /// so a transposition cannot be silent.
    ///
    /// Weights are never embedded in the `.wasm`; the host fetches them.
    ///
    /// # Errors
    ///
    /// Returns a JS error if a field is missing, is not a byte buffer, is
    /// empty, or if the bytes do not parse as a GLiNER2 model.
    pub async fn load(options: JsValue) -> Result<NerModel, JsValue> {
        if !options.is_object() {
            return Err(js_from_any(
                "NerModel.load expects an object with `weights`, `tokenizer`, and `encoderConfig` byte buffers",
            ));
        }

        let weights = required_bytes(&options, "weights")?;
        let tokenizer = required_bytes(&options, "tokenizer")?;
        let encoder_config = required_bytes(&options, "encoderConfig")?;

        let backend = CandleBackend::from_bytes(&weights, &tokenizer, &encoder_config)
            .map_err(|e| js_from_any(format!("NerModel.load: {e}")))?;

        Ok(NerModel { backend })
    }

    /// Detect entities in `text`, running inference inside the binary.
    ///
    /// `opts` may contain `categories`, an array of category names; unknown
    /// names become custom zero-shot labels. Omitting it uses the backend's
    /// default label set.
    ///
    /// Resolves to an array of `{ category, text, start, end, confidence }`,
    /// with `start` and `end` as byte offsets into `text`.
    ///
    /// # Errors
    ///
    /// Returns a JS error if inference fails.
    pub async fn detect(&self, text: String, opts: JsValue) -> Result<JsValue, JsValue> {
        let categories = categories_from_opts(&opts);

        let entities = self
            .backend
            .detect(&text, &categories)
            .await
            .map_err(|e| js_from_any(format!("NerModel.detect: {e}")))?;

        serde_wasm_bindgen::to_value(&entities).map_err(|e| js_from_any(e.to_string()))
    }
}

/// In-crate tests, run under Node via `scripts/ci/wasm/run-crate-tests.sh`.
///
/// Argument contract and failure paths only; real weights are too large for CI.
#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use wasm_bindgen_test::*;

    use super::*;

    fn eval(src: &str) -> JsValue {
        js_sys::eval(src).expect("test JS snippet must evaluate")
    }

    fn err_text(err: JsValue) -> String {
        err.as_string().unwrap_or_else(|| format!("{err:?}"))
    }

    #[wasm_bindgen_test]
    async fn load_rejects_non_object() {
        let err = NerModel::load(JsValue::from_f64(42.0)).await.err().expect("must fail");
        assert!(err_text(err).contains("expects an object"));
    }

    #[wasm_bindgen_test]
    async fn load_names_the_missing_field() {
        let err = NerModel::load(eval("({})")).await.err().expect("must fail");
        let text = err_text(err);
        assert!(text.contains("weights"), "error must name the field: {text}");
    }

    #[wasm_bindgen_test]
    async fn load_names_a_later_missing_field() {
        let options = eval("({ weights: new Uint8Array([1, 2, 3]) })");
        let err = NerModel::load(options).await.err().expect("must fail");
        let text = err_text(err);
        assert!(text.contains("tokenizer"), "error must name the field: {text}");
    }

    #[wasm_bindgen_test]
    async fn load_rejects_wrong_type_by_name() {
        let options = eval("({ weights: 'not bytes', tokenizer: new Uint8Array([1]), encoderConfig: new Uint8Array([1]) })");
        let err = NerModel::load(options).await.err().expect("must fail");
        let text = err_text(err);
        assert!(text.contains("weights") && text.contains("Uint8Array"), "got: {text}");
    }

    #[wasm_bindgen_test]
    async fn load_rejects_empty_buffer() {
        let options =
            eval("({ weights: new Uint8Array([]), tokenizer: new Uint8Array([1]), encoderConfig: new Uint8Array([1]) })");
        let err = NerModel::load(options).await.err().expect("must fail");
        assert!(err_text(err).contains("empty"));
    }

    #[wasm_bindgen_test]
    async fn load_accepts_array_buffer_and_fails_on_content_not_type() {
        // ArrayBuffer must pass the type check and fail on the bytes instead. ~keep
        let options = eval(
            "({ weights: new ArrayBuffer(8), tokenizer: new ArrayBuffer(8), encoderConfig: new ArrayBuffer(8) })",
        );
        let err = NerModel::load(options).await.err().expect("must fail");
        let text = err_text(err);
        assert!(text.contains("NerModel.load:"), "expected a parse failure, got: {text}");
        assert!(!text.contains("must be a Uint8Array"), "ArrayBuffer must be accepted: {text}");
    }

    #[wasm_bindgen_test]
    async fn load_reports_malformed_model_input_without_trapping() {
        // Malformed bytes must surface as a JS error, never a panic or wasm trap.
        // `from_bytes` parses the tokenizer first, so this stops there rather than
        // reaching the safetensors loader; that path needs a valid tokenizer blob,
        // too large to inline. ~keep
        let options = eval(
            "({ weights: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]), tokenizer: new Uint8Array([123, 125]), encoderConfig: new Uint8Array([123, 125]) })",
        );
        let err = NerModel::load(options).await.err().expect("must fail");
        let text = err_text(err);
        assert!(text.contains("NerModel.load:"), "must be a load failure, got: {text}");
    }
}
