//! NER backend backed by `xberg-gliner`'s `candle` module (GLiNER2 safetensors + optional LoRA).

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
use std::path::PathBuf;
use std::sync::Mutex;
#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
use std::sync::{Arc, LazyLock};

#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
use ahash::AHashMap;
use async_trait::async_trait;
#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
use parking_lot::RwLock;
use xberg_gliner::candle::Gliner2Candle;

use crate::Result;
use crate::text::ner::NerBackend;
use crate::types::entity::{Entity, EntityCategory};

const DEFAULT_THRESHOLD: f32 = 0.5;

/// Cache key: `(model_dir, lora_adapter_dir)`; mirrors `gline::get_or_init_backend`'s
/// cache-by-source-and-config pattern (not shared with it directly: `gline` is gated by
/// the independent `ner-onnx` feature, so a `ner-candle`-only build must not depend on
/// it) so a given (model, adapter) pair is loaded and LoRA-merged at most once per
/// process, instead of on every `process()` call.
#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
type CandleBackendCacheKey = (PathBuf, Option<PathBuf>);

#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
static CANDLE_BACKEND_CACHE: LazyLock<RwLock<AHashMap<CandleBackendCacheKey, Arc<CandleBackend>>>> =
    LazyLock::new(|| RwLock::new(AHashMap::default()));

/// Return the cached backend for `key`, or build and cache one via `build`.
#[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
fn get_or_insert_arc(
    key: CandleBackendCacheKey,
    build: impl FnOnce() -> crate::Result<CandleBackend>,
) -> crate::Result<Arc<CandleBackend>> {
    {
        let cache = CANDLE_BACKEND_CACHE.read();
        if let Some(value) = cache.get(&key) {
            tracing::debug!(model_dir = %key.0.display(), "Candle GLiNER2 backend found in cache");
            return Ok(Arc::clone(value));
        }
    }

    let mut cache = CANDLE_BACKEND_CACHE.write();
    if let Some(value) = cache.get(&key) {
        return Ok(Arc::clone(value));
    }

    let value = Arc::new(build()?);
    cache.insert(key, Arc::clone(&value));
    Ok(value)
}

/// Wraps [`Gliner2Candle`] behind the [`NerBackend`] trait.
///
/// `Gliner2Candle` holds candle tensors which are not `Send`, so we wrap it
/// in a `Mutex` to satisfy the `Send + Sync` requirement of [`NerBackend`].
#[cfg_attr(alef, alef(skip))] // binding surface arrives with the NER dispatch follow-up ~keep
pub struct CandleBackend {
    model: Mutex<Gliner2Candle>,
}

impl CandleBackend {
    /// Load from a local model directory. Applies `lora_adapter_dir` if provided.
    ///
    /// `model_dir` must contain `tokenizer.json` and `model.safetensors`.
    /// `lora_adapter_dir`, when set, must contain `adapter_config.json` and
    /// `adapter_model.safetensors`; merged into the base weights at load time.
    ///
    /// Not available on `wasm32`; `Gliner2Candle::from_local` requires filesystem
    /// access, which wasm32 does not have. Use [`Self::from_bytes`] there instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_local(model_dir: &Path, lora_adapter_dir: Option<&Path>) -> crate::Result<Self> {
        let mut model = Gliner2Candle::from_local(model_dir).map_err(|e| crate::XbergError::Plugin {
            message: format!("CandleBackend load: {e}"),
            plugin_name: "ner-candle".to_string(),
        })?;
        if let Some(adapter_dir) = lora_adapter_dir {
            let adapter_name = adapter_dir.file_name().and_then(|n| n.to_str()).unwrap_or("adapter");
            model
                .load_adapter(adapter_name, adapter_dir)
                .map_err(|e| crate::XbergError::Plugin {
                    message: format!("CandleBackend load_adapter: {e}"),
                    plugin_name: "ner-candle".to_string(),
                })?;
        }
        tracing::info!(
            model_dir = %model_dir.display(),
            adapter = ?lora_adapter_dir.map(std::path::Path::display),
            "Candle GLiNER2 backend loaded"
        );
        Ok(Self {
            model: Mutex::new(model),
        })
    }

    /// Load from a local model directory, reusing a cached, already-LoRA-merged
    /// backend when one exists for the same `(model_dir, lora_adapter_dir)` pair.
    ///
    /// Unlike [`Self::from_local`], which reloads and re-merges on every call, this
    /// is the entry point [`crate::plugins::processor::builtin::ner::make_backend`]
    /// should use so a document-processing pipeline pays the model-load + LoRA-merge
    /// cost once per (model, adapter) pair, not once per document.
    #[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
    pub fn get_or_init(model_dir: &Path, lora_adapter_dir: Option<&Path>) -> crate::Result<Arc<Self>> {
        let key: CandleBackendCacheKey = (model_dir.to_path_buf(), lora_adapter_dir.map(Path::to_path_buf));
        get_or_insert_arc(key, || Self::from_local(model_dir, lora_adapter_dir))
    }

    /// Load from in-memory model bytes (no filesystem access; required on `wasm32`,
    /// also usable natively when the caller already has the model bytes in memory).
    pub fn from_bytes(safetensors: &[u8], tokenizer_json: &[u8], encoder_config_json: &[u8]) -> crate::Result<Self> {
        let model = Gliner2Candle::from_bytes(safetensors, tokenizer_json, encoder_config_json).map_err(|e| {
            crate::XbergError::Plugin {
                message: format!("CandleBackend load: {e}"),
                plugin_name: "ner-candle".to_string(),
            }
        })?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }
}

fn spans_to_entities(spans: Vec<xberg_gliner::Span>) -> Vec<Entity> {
    spans
        .into_iter()
        .map(|span| {
            let (start, end) = span.offsets();
            Entity {
                category: EntityCategory::from(span.class().to_string()),
                text: span.text().to_string(),
                start: start as u32,
                end: end as u32,
                confidence: Some(span.probability()),
            }
        })
        .collect()
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl NerBackend for CandleBackend {
    async fn detect(&self, text: &str, categories: &[EntityCategory]) -> Result<Vec<Entity>> {
        let labels: Vec<&str> = if categories.is_empty() {
            default_labels().to_vec()
        } else {
            categories.iter().map(category_to_label).collect()
        };

        let model = self.model.lock().map_err(|_| crate::XbergError::Plugin {
            message: "CandleBackend: model mutex poisoned".to_string(),
            plugin_name: "ner-candle".to_string(),
        })?;

        // extract_ner is CPU-bound and already synchronous; block_in_place lets tokio move
        // other tasks off this thread without requiring Send.
        //
        // Gated on tokio's presence, not the target: `ner-candle-wasm` omits `tokio-runtime`
        // and builds natively too, so a `target_arch` gate alone would reach `block_in_place`
        // with no tokio in the graph. The arms are exact complements. ~keep
        #[cfg(all(not(target_arch = "wasm32"), feature = "tokio-runtime"))]
        let spans =
            tokio::task::block_in_place(|| model.extract_ner(text, &labels, DEFAULT_THRESHOLD)).map_err(|e| {
                crate::XbergError::Plugin {
                    message: format!("CandleBackend inference: {e}"),
                    plugin_name: "ner-candle".to_string(),
                }
            })?;

        #[cfg(not(all(not(target_arch = "wasm32"), feature = "tokio-runtime")))]
        let spans = model
            .extract_ner(text, &labels, DEFAULT_THRESHOLD)
            .map_err(|e| crate::XbergError::Plugin {
                message: format!("CandleBackend inference: {e}"),
                plugin_name: "ner-candle".to_string(),
            })?;

        Ok(spans_to_entities(spans))
    }
}

/// Labels used when the caller supplies an empty `categories` slice; matches the
/// full default set the other NER backends (ONNX/LLM) use, not a narrower subset.
fn default_labels() -> &'static [&'static str] {
    &[
        "person",
        "organization",
        "location",
        "email",
        "phone",
        "date",
        "time",
        "money",
        "percent",
        "url",
    ]
}

fn category_to_label(cat: &EntityCategory) -> &str {
    match cat {
        EntityCategory::Person => "person",
        EntityCategory::Organization => "organization",
        EntityCategory::Location => "location",
        EntityCategory::Email => "email",
        EntityCategory::Phone => "phone",
        EntityCategory::Date => "date",
        EntityCategory::Time => "time",
        EntityCategory::Money => "money",
        EntityCategory::Percent => "percent",
        EntityCategory::Url => "url",
        EntityCategory::Custom(s) => s.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_to_label_maps_known_categories() {
        assert_eq!(category_to_label(&EntityCategory::Person), "person");
        assert_eq!(category_to_label(&EntityCategory::Organization), "organization");
        assert_eq!(category_to_label(&EntityCategory::Location), "location");
        assert_eq!(category_to_label(&EntityCategory::Email), "email");
        assert_eq!(category_to_label(&EntityCategory::Phone), "phone");
        assert_eq!(category_to_label(&EntityCategory::Date), "date");
        assert_eq!(category_to_label(&EntityCategory::Time), "time");
        assert_eq!(category_to_label(&EntityCategory::Money), "money");
        assert_eq!(category_to_label(&EntityCategory::Percent), "percent");
        assert_eq!(category_to_label(&EntityCategory::Url), "url");
        assert_eq!(
            category_to_label(&EntityCategory::Custom("product".to_string())),
            "product"
        );
    }

    #[test]
    fn from_bytes_rejects_empty_input() {
        let result = CandleBackend::from_bytes(&[], b"{}", b"{}");
        assert!(result.is_err());
    }

    #[test]
    fn default_labels_matches_broader_backend_set() {
        // Must include the full default category set the other NER backends use,
        // not just the narrower person/organization/location/email/phone subset. ~keep
        let labels = default_labels();
        for expected in [
            "person",
            "organization",
            "location",
            "email",
            "phone",
            "date",
            "time",
            "money",
            "percent",
            "url",
        ] {
            assert!(labels.contains(&expected), "missing default label: {expected}");
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "ner-candle-backend"))]
    #[test]
    fn get_or_init_propagates_load_errors_without_panicking() {
        let missing_dir = std::path::Path::new("/nonexistent/xberg-candle-cache-test-model-dir");
        let result = CandleBackend::get_or_init(missing_dir, None);
        assert!(result.is_err());
    }

    #[test]
    fn spans_to_entities_is_empty_for_no_spans() {
        let entities = spans_to_entities(vec![]);
        assert!(entities.is_empty());
    }

    #[test]
    fn spans_to_entities_converts_fields_correctly() {
        let span =
            xberg_gliner::Span::new(0, 0, 5, "Alice".to_string(), "person".to_string(), 0.92).expect("valid span");
        let entities = spans_to_entities(vec![span]);

        assert_eq!(entities.len(), 1);
        let e = &entities[0];
        assert_eq!(e.text, "Alice");
        assert_eq!(e.category, EntityCategory::Person);
        assert_eq!(e.start, 0);
        assert_eq!(e.end, 5);
        assert!((e.confidence.unwrap() - 0.92).abs() < 1e-5);
    }
}
