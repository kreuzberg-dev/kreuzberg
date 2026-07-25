//! `XbergEngine`; a stateful handle carrying injected bridges and
//! convenience methods on top of the raw WASM exports.

use js_sys::Object;
use wasm_bindgen::prelude::*;

use crate::bridge::ner::resolve_ner_with_timeout;
use crate::bridge::ocr::resolve_ocr_with_timeout;

/// Extract an optional JS object field, returning `None` if the field is
/// missing, `null`, or `undefined`.
fn get_opt_field(obj: &Object, field: &str) -> Result<Option<Object>, JsValue> {
    let val = js_sys::Reflect::get(obj, &JsValue::from_str(field))
        .map_err(|_| JsValue::from_str(&format!("failed to read field '{field}'")))?;
    if val.is_undefined() || val.is_null() {
        return Ok(None);
    }
    val.dyn_into::<Object>()
        .map(Some)
        .map_err(|_| JsValue::from_str(&format!("field '{field}' must be an object")))
}

/// Extract an optional numeric field, returning `None` if the field is
/// missing, `null`, `undefined`, or not a number.
fn get_opt_number(obj: &Object, field: &str) -> Result<Option<f64>, JsValue> {
    let val = js_sys::Reflect::get(obj, &JsValue::from_str(field))
        .map_err(|_| JsValue::from_str(&format!("failed to read field '{field}'")))?;
    if val.is_undefined() || val.is_null() {
        return Ok(None);
    }
    Ok(val.as_f64())
}

/// Stateful engine handle exposed to JS.
///
/// Constructed via `XbergEngine.new(config, injection)` where `config` may
/// contain optional settings (e.g. `bridgeTimeoutMs`) and `injection` is a
/// plain object with an optional `ocr` key.
#[wasm_bindgen]
pub struct XbergEngine {
    ner: Option<js_sys::Object>,
    ocr: Option<js_sys::Object>,
    bridge_timeout_ms: u32,
}

#[wasm_bindgen]
impl XbergEngine {
    /// Create a new engine with injected bridges.
    ///
    /// `config` may contain:
    /// - `bridgeTimeoutMs`; timeout in milliseconds for JS bridge calls
    ///   (defaults to 30,000ms if not provided)
    ///
    /// `injection` may contain:
    /// - `ner`; object with `ner(text, categories): Promise<Array<{ category, text, start, end, confidence? }>>`
    /// - `ocr`; object with `ocr(imageBytes, opts): Promise<{ text: string, lines?: Array<{ text: string, confidence: number, bbox?: { x: number, y: number, width: number, height: number } }> }>`
    ///
    /// Unknown injection keys are ignored, so hosts can pass richer injection
    /// objects shared with other engines.
    #[wasm_bindgen(constructor)]
    pub fn new(config: JsValue, injection: JsValue) -> Result<XbergEngine, JsValue> {
        let bridge_timeout_ms = if config.is_undefined() || config.is_null() {
            crate::bridge::BRIDGE_TIMEOUT_MS
        } else {
            let config_obj: Object = config
                .dyn_into()
                .map_err(|_| JsValue::from_str("config must be an object"))?;
            match get_opt_number(&config_obj, "bridgeTimeoutMs")? {
                Some(v) => {
                    if !v.is_finite() || v < 0.0 {
                        return Err(JsValue::from_str(
                            "bridgeTimeoutMs must be a non-negative, finite number",
                        ));
                    }
                    v as u32
                }
                None => crate::bridge::BRIDGE_TIMEOUT_MS,
            }
        };

        let obj: Object = if injection.is_undefined() || injection.is_null() {
            Object::new()
        } else {
            injection
                .dyn_into::<Object>()
                .map_err(|_| JsValue::from_str("injection must be an object"))?
        };

        let ner = get_opt_field(&obj, "ner")?;
        let ocr = get_opt_field(&obj, "ocr")?;

        Ok(XbergEngine {
            ner,
            ocr,
            bridge_timeout_ms,
        })
    }

    /// Extract content from a single bytes or URI input.
    #[allow(clippy::missing_errors_doc)]
    pub async fn extract(&self, input: JsValue, config: JsValue) -> Result<JsValue, JsValue> {
        let input_core: xberg::ExtractInput = if input.is_undefined() {
            xberg::ExtractInput::default()
        } else {
            serde_wasm_bindgen::from_value::<xberg::ExtractInput>(input)
                .map_err(|e| JsValue::from_str(&e.to_string()))?
        };
        let config_core: xberg::ExtractionConfig = if config.is_undefined() {
            xberg::ExtractionConfig::default()
        } else {
            serde_wasm_bindgen::from_value::<xberg::ExtractionConfig>(config)
                .map_err(|e| JsValue::from_str(&e.to_string()))?
        };
        let result = xberg::extract(input_core, &config_core)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let wasm_result = crate::WasmExtractionResult::from(result);
        Ok(wasm_result.into())
    }

    /// Perform Named Entity Recognition on `text` through the injected NER
    /// backend. `opts` may contain `categories`, an array of category names;
    /// unknown names are treated as custom zero-shot labels.
    ///
    /// This is the injected-backend path. To run a model inside the browser
    /// instead, load one with
    /// [`NerModel`](crate::bridge::ner_model::NerModel) and call its `detect`
    /// method directly — it needs none of the promise bridging or timeout this
    /// path provides.
    #[allow(clippy::missing_errors_doc)]
    pub async fn ner(&self, text: String, opts: JsValue) -> Result<JsValue, JsValue> {
        let categories = crate::bridge::ner::categories_from_opts(&opts);

        let entities = resolve_ner_with_timeout(self.ner.clone(), &text, &categories, self.bridge_timeout_ms)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:?}")))?;

        serde_wasm_bindgen::to_value(&entities).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Perform OCR on image bytes, returning extracted text with per-line
    /// confidence and bounding-box geometry (when the backend provides it).
    #[allow(clippy::missing_errors_doc)]
    pub async fn ocr(&self, bytes: Vec<u8>, opts: JsValue) -> Result<JsValue, JsValue> {
        let language = if opts.is_undefined() || opts.is_null() {
            "eng".to_string()
        } else {
            js_sys::Reflect::get(&opts, &JsValue::from_str("language"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| "eng".to_string())
        };

        let result = resolve_ocr_with_timeout(self.ocr.clone(), &bytes, &language, self.bridge_timeout_ms)
            .await
            .map_err(|e| JsValue::from_str(&format!("{e:?}")))?;

        serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

/// Detect page layout (RT-DETR) from encoded image bytes using a caller-supplied
/// ONNX model.
///
/// Both arguments are raw bytes: `imageBytes` is an encoded image (PNG/JPEG/…) and
/// `modelBytes` is the RT-DETR `.onnx` weights the JS host fetched. Weights are
/// never embedded in the `.wasm` (RT-DETR alone runs to hundreds of MB, far over
/// the CDN per-file cap), so the host fetches them and hands the bytes over here.
/// Inference runs entirely in Rust through the pure-Rust tract engine; the returned
/// value is a `DetectionResult` object (bounding boxes, classes, confidences).
///
/// Only RT-DETR detection is available on WASM; the ORT-only layout models
/// (`PP-DocLayout-V3`, YOLO) and table-structure recognition (TATR, SLANeXT) are not.
#[wasm_bindgen(js_name = "detectLayout")]
#[allow(clippy::missing_errors_doc)]
pub fn detect_layout(image_bytes: Vec<u8>, model_bytes: Vec<u8>) -> Result<JsValue, JsValue> {
    let mut engine = xberg::layout::LayoutEngine::from_rtdetr_bytes(&model_bytes, None)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let result = engine
        .detect_image_bytes(&image_bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Detect document page orientation (PP-LCNet) from encoded image bytes using a
/// caller-supplied ONNX model.
///
/// See [`detect_layout`] for the bytes contract. Returns an `OrientationResult`
/// object with the detected rotation (0/90/180/270 degrees) and its confidence.
#[wasm_bindgen(js_name = "detectOrientation")]
#[allow(clippy::missing_errors_doc)]
pub fn detect_orientation(image_bytes: Vec<u8>, model_bytes: Vec<u8>) -> Result<JsValue, JsValue> {
    let detector = xberg::doc_orientation::DocOrientationDetector::from_bytes(model_bytes, None);
    let result = detector
        .detect_image_bytes(&image_bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// In-crate tests for the engine's bridge surface, run under Node via
/// `scripts/ci/wasm/run-crate-tests.sh` (which wraps `wasm-pack test --node`
/// with the import shims from `test-shims/`).
///
/// They live in this hand-written module rather than `tests/` because the
/// generated manifest builds only a `cdylib`, which integration tests cannot
/// link against. They run under Node rather than a browser because the
/// wasm-bindgen test runner's glue carries the same unresolvable `env` /
/// `wasi_snapshot_preview1` imports that `scripts/fix-wasi-imports.mjs`
/// patches out of the published package; under Node those modules can be
/// supplied via `NODE_PATH`, while the browser ESM loader offers no hook.
/// Nothing here needs a DOM.
///
/// The vitest suites under `e2e/wasm/tests/` exercise the same contract from
/// the JS side, against the built npm package.
#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use wasm_bindgen_test::*;

    use super::*;

    /// Evaluate a JS expression and return the resulting value.
    fn eval(src: &str) -> JsValue {
        js_sys::eval(src).expect("test JS snippet must evaluate")
    }

    /// Build an engine from JS expressions for the config and injection objects.
    fn engine_from(config_src: &str, injection_src: &str) -> XbergEngine {
        XbergEngine::new(eval(config_src), eval(injection_src)).expect("engine construction must succeed")
    }

    /// Read `field` off a JS object.
    fn get(obj: &JsValue, field: &str) -> JsValue {
        js_sys::Reflect::get(obj, &JsValue::from_str(field)).expect("field read must succeed")
    }

    /// Render an engine-method error into a `String` for message asserts.
    fn err_text(err: JsValue) -> String {
        err.as_string().unwrap_or_else(|| format!("{err:?}"))
    }

    #[wasm_bindgen_test]
    fn construction_with_empty_objects() {
        assert!(XbergEngine::new(eval("({})"), eval("({})")).is_ok());
    }

    #[wasm_bindgen_test]
    fn construction_with_null_config_and_injection() {
        assert!(XbergEngine::new(JsValue::NULL, JsValue::NULL).is_ok());
    }

    #[wasm_bindgen_test]
    fn construction_rejects_non_object_injection() {
        let result = XbergEngine::new(eval("({})"), JsValue::from_f64(42.0));
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("injection must be an object"));
    }

    #[wasm_bindgen_test]
    fn construction_rejects_negative_timeout() {
        let result = XbergEngine::new(eval("({ bridgeTimeoutMs: -1 })"), JsValue::NULL);
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("bridgeTimeoutMs"));
    }

    #[wasm_bindgen_test]
    async fn ocr_without_injection_errors() {
        let engine = engine_from("({})", "({})");
        let result = engine.ocr(vec![0u8; 4], JsValue::UNDEFINED).await;
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("OCR unavailable"));
    }

    #[wasm_bindgen_test]
    async fn ocr_with_empty_bytes_errors() {
        let engine = engine_from("({})", "({})");
        let result = engine.ocr(Vec::new(), JsValue::UNDEFINED).await;
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("empty"));
    }

    #[wasm_bindgen_test]
    async fn ocr_injection_without_function_errors() {
        let engine = engine_from("({})", "({ ocr: {} })");
        let result = engine.ocr(vec![0u8; 4], JsValue::UNDEFINED).await;
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("no 'ocr' function"));
    }

    #[wasm_bindgen_test]
    async fn ocr_roundtrips_text_lines_and_geometry() {
        let engine = engine_from(
            "({})",
            r#"({
                ocr: {
                    ocr: (bytes, opts) => Promise.resolve({
                        text: "hello world",
                        lines: [
                            { text: "hello", confidence: 0.9, bbox: { x: 1, y: 2, width: 30, height: 10 } },
                            { text: "world", confidence: 0.8 },
                        ],
                    }),
                },
            })"#,
        );

        let result = engine
            .ocr(vec![0u8; 4], JsValue::UNDEFINED)
            .await
            .expect("ocr must succeed");
        assert_eq!(get(&result, "text").as_string().unwrap(), "hello world");

        let lines: js_sys::Array = get(&result, "lines").dyn_into().unwrap();
        assert_eq!(lines.length(), 2);

        let first = lines.get(0);
        assert_eq!(get(&first, "text").as_string().unwrap(), "hello");
        let bbox = get(&first, "bbox");
        assert_eq!(get(&bbox, "width").as_f64().unwrap(), 30.0);

        // Line without geometry stays geometry-free rather than erroring.
        let second = lines.get(1);
        assert_eq!(get(&second, "text").as_string().unwrap(), "world");
        let second_bbox = get(&second, "bbox");
        assert!(second_bbox.is_undefined() || second_bbox.is_null());
    }

    #[wasm_bindgen_test]
    async fn ocr_missing_lines_degrades_to_empty() {
        let engine = engine_from(
            "({})",
            r#"({ ocr: { ocr: () => Promise.resolve({ text: "just text" }) } })"#,
        );

        let result = engine
            .ocr(vec![0u8; 4], JsValue::UNDEFINED)
            .await
            .expect("ocr must succeed");
        assert_eq!(get(&result, "text").as_string().unwrap(), "just text");
        let lines: js_sys::Array = get(&result, "lines").dyn_into().unwrap();
        assert_eq!(lines.length(), 0);
    }

    #[wasm_bindgen_test]
    async fn ocr_forwards_language_option() {
        let engine = engine_from(
            "({})",
            r#"({
                ocr: {
                    ocr: (bytes, opts) => {
                        globalThis.__xbergTestOcrLanguage = opts.language;
                        return Promise.resolve({ text: "" });
                    },
                },
            })"#,
        );

        engine
            .ocr(vec![0u8; 4], eval("({ language: 'deu' })"))
            .await
            .expect("ocr must succeed");
        let seen = eval("globalThis.__xbergTestOcrLanguage");
        assert_eq!(seen.as_string().unwrap(), "deu");

        engine
            .ocr(vec![0u8; 4], JsValue::UNDEFINED)
            .await
            .expect("ocr must succeed");
        let seen_default = eval("globalThis.__xbergTestOcrLanguage");
        assert_eq!(seen_default.as_string().unwrap(), "eng");
    }

    #[wasm_bindgen_test]
    async fn ner_without_injection_errors() {
        let engine = engine_from("({})", "({})");
        let result = engine.ner("some text".to_string(), JsValue::UNDEFINED).await;
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("NER unavailable"));
    }

    #[wasm_bindgen_test]
    async fn ner_roundtrips_entities() {
        let engine = engine_from(
            "({})",
            r#"({
                ner: {
                    ner: (text, categories) => Promise.resolve([
                        { category: "email", text: "alice@example.com", start: 8, end: 25, confidence: 0.95 },
                    ]),
                },
            })"#,
        );

        let result = engine
            .ner("Contact alice@example.com".to_string(), JsValue::UNDEFINED)
            .await
            .expect("ner must succeed");
        let entities: js_sys::Array = result.dyn_into().unwrap();
        assert_eq!(entities.length(), 1);

        let entity = entities.get(0);
        assert_eq!(get(&entity, "category").as_string().unwrap(), "email");
        assert_eq!(get(&entity, "text").as_string().unwrap(), "alice@example.com");
        assert_eq!(get(&entity, "start").as_f64().unwrap(), 8.0);
        assert_eq!(get(&entity, "end").as_f64().unwrap(), 25.0);
    }

    #[wasm_bindgen_test]
    async fn ner_sends_category_wire_names() {
        let engine = engine_from(
            "({})",
            r#"({
                ner: {
                    ner: (text, categories) => {
                        globalThis.__xbergTestNerCategories = categories;
                        return Promise.resolve([]);
                    },
                },
            })"#,
        );

        engine
            .ner(
                "text".to_string(),
                eval("({ categories: ['email', 'invoice_number'] })"),
            )
            .await
            .expect("ner must succeed");

        let seen: js_sys::Array = eval("globalThis.__xbergTestNerCategories").dyn_into().unwrap();
        // Built-in categories keep their snake_case names; unknown names pass
        // through as custom zero-shot labels.
        assert_eq!(seen.length(), 2);
        assert_eq!(seen.get(0).as_string().unwrap(), "email");
        assert_eq!(seen.get(1).as_string().unwrap(), "invoice_number");
    }

    #[wasm_bindgen_test]
    async fn bridge_timeout_rejects_hung_backend() {
        let engine = engine_from(
            "({ bridgeTimeoutMs: 50 })",
            r#"({ ocr: { ocr: () => new Promise(() => {}) } })"#,
        );

        let result = engine.ocr(vec![0u8; 4], JsValue::UNDEFINED).await;
        assert!(result.is_err());
        assert!(err_text(result.err().unwrap()).contains("timed out"));
    }

    /// The injection object is `this` for the bridge call, so backends written
    /// as class instances with method syntax keep their internal state.
    #[wasm_bindgen_test]
    async fn bridge_preserves_backend_this_binding() {
        let engine = engine_from(
            "({})",
            r#"({
                ocr: {
                    prefix: "seen:",
                    ocr(bytes, opts) {
                        return Promise.resolve({ text: this.prefix + opts.language });
                    },
                },
            })"#,
        );

        let result = engine
            .ocr(vec![0u8; 4], JsValue::UNDEFINED)
            .await
            .expect("ocr must succeed");
        assert_eq!(get(&result, "text").as_string().unwrap(), "seen:eng");
    }

    // Layout/orientation run entirely in Rust (tract) on caller-supplied ONNX
    // bytes. Exercising them against real RT-DETR/PP-LCNet weights needs
    // hundreds of MB of models fetched at runtime, which the JS-side vitest e2e
    // covers; here we assert the wired byte path rejects invalid model bytes
    // gracefully (a JS error, never a wasm panic/trap).

    #[wasm_bindgen_test]
    fn detect_layout_rejects_invalid_model_bytes() {
        let result = detect_layout(vec![0u8; 8], b"not an onnx model".to_vec());
        assert!(result.is_err());
    }

    #[wasm_bindgen_test]
    fn detect_orientation_errors_on_invalid_input() {
        // Orientation loads its model lazily, so this fails at image decode;
        // either way the wired path must surface an error, not trap.
        let result = detect_orientation(vec![0u8; 8], b"not an onnx model".to_vec());
        assert!(result.is_err());
    }
}
