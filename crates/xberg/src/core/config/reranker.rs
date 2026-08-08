//! Reranker configuration types.
//!
//! Configuration for cross-encoder reranking, which scores `(query, document)` pairs
//! to reorder candidate documents by relevance. Three backend variants are supported:
//! local ONNX cross-encoder, provider-hosted via liter-llm, and an in-process plugin.
//!
//! Since v5.0.0.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::llm::LlmConfig;

/// Configuration for the reranking pipeline.
///
/// Controls which model to use, how many results to return, and download/cache
/// behavior for local ONNX models.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankerConfig {
    /// The reranker model to use (defaults to "balanced" preset if not specified).
    #[serde(default = "default_reranker_model", deserialize_with = "deserialize_null_model")]
    pub model: RerankerModelType,

    /// Return at most this many documents. `None` returns all.
    ///
    /// Applied after sorting by score, so the highest-scoring documents are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,

    /// Batch size for local ONNX cross-encoder inference.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Show model download progress (local ONNX path only).
    ///
    /// When enabled, transfer progress for the model, tokenizer and config files is reported at
    /// `info` level on the `xberg::model_download` target while they download (#279). A warm
    /// Hugging Face cache transfers nothing and so reports nothing. Ignored by
    /// [`RerankerModelType::Llm`] and [`RerankerModelType::Plugin`], which download no model.
    #[serde(default)]
    pub show_download_progress: bool,

    /// Optional alternate Hugging Face cache root for model files.
    ///
    /// When unset, hf-hub follows the standard Hugging Face environment and
    /// platform cache conventions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Hardware acceleration for the reranker ONNX model.
    ///
    /// Controls which execution provider (CPU, CUDA, CoreML, TensorRT) is used for
    /// local inference. Defaults to `None` (auto-select per platform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Maximum wall-clock duration (in seconds) for a single `rerank()` call when
    /// using [`RerankerModelType::Plugin`].
    ///
    /// Applies only to the in-process plugin path — protects against hung
    /// host-language backends. On timeout, the dispatcher returns
    /// [`crate::XbergError::Plugin`] instead of blocking forever.
    ///
    /// `None` disables the timeout. The default (60 seconds) is conservative
    /// for common in-process inference; increase for large document sets on slow
    /// hardware.
    #[serde(
        default = "default_max_rerank_duration_secs",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_rerank_duration_secs: Option<u64>,
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            model: RerankerModelType::Preset {
                name: "balanced".to_string(),
            },
            top_k: None,
            batch_size: 32,
            show_download_progress: false,
            cache_dir: None,
            acceleration: None,
            max_rerank_duration_secs: Some(60),
        }
    }
}

/// Selects how a local ONNX reranker's raw output tensor is turned into a score.
///
/// - [`RerankerHead::CrossEncoder`] — classic single-logit cross-encoder head:
///   the model emits `[batch, 1]` (or `[batch]`) logits; the caller applies
///   sigmoid to get a `[0, 1]` score. This is the original, unchanged path.
/// - [`RerankerHead::Qwen3Generative`] — Qwen3 generative-reranker head: the
///   model emits `[batch, seq, vocab]` logits; the score is `P("yes")` read
///   from the last token's logits over the "yes"/"no" vocabulary entries,
///   via a softmax over those two logits. Already a `[0, 1]` probability —
///   no sigmoid is applied.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankerHead {
    /// Single-logit cross-encoder head (sigmoid applied by the caller).
    CrossEncoder,
    /// Qwen3 generative-reranker head (softmax over yes/no token logits).
    Qwen3Generative,
}

impl Default for RerankerHead {
    /// Returns [`RerankerHead::CrossEncoder`], the original scoring path.
    fn default() -> Self {
        Self::CrossEncoder
    }
}

/// Serialize the head as its serde `snake_case` tag.
///
/// The polyglot bindings represent this unit enum as a string when it appears
/// as a field of the tagged [`RerankerModelType::Custom`] variant; the generated
/// glue calls `.into()` to cross the FFI boundary, so both directions of the
/// `String` conversion must exist.
impl From<RerankerHead> for String {
    fn from(head: RerankerHead) -> Self {
        match head {
            RerankerHead::CrossEncoder => "cross_encoder".to_string(),
            RerankerHead::Qwen3Generative => "qwen3_generative".to_string(),
        }
    }
}

/// Parse the head from its serde `snake_case` tag.
///
/// Unknown values fall back to [`RerankerHead::CrossEncoder`] rather than
/// erroring, matching the `.unwrap_or_default()` the binding glue applies. This
/// is the inverse of [`From<RerankerHead>`] for [`String`].
impl From<String> for RerankerHead {
    fn from(value: String) -> Self {
        match value.as_str() {
            "qwen3_generative" => RerankerHead::Qwen3Generative,
            _ => RerankerHead::CrossEncoder,
        }
    }
}

/// Reranker model types supported by Xberg.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RerankerModelType {
    /// Use a preset cross-encoder model (recommended).
    Preset {
        /// Preset name (e.g. "balanced", "fast", "quality", "multilingual").
        name: String,
    },

    /// Use a custom ONNX cross-encoder from HuggingFace.
    Custom {
        /// HuggingFace model repository ID (e.g. "cross-encoder/ms-marco-MiniLM-L6-v2").
        model_id: String,
        /// Path to the ONNX file within the repo.
        ///
        /// Defaults to `"onnx/model.onnx"` when `None`. Override for repos that
        /// place the weight elsewhere (e.g. `"model.onnx"` for `rozgo/bge-reranker-v2-m3`,
        /// `"onnx/model_quantized.onnx"` for int8 variants).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_file: Option<String>,
        /// Sibling files that must be downloaded alongside `model_file`.
        ///
        /// Empty for most repos. Set to e.g. `vec!["model.onnx.data".into()]` for
        /// `rozgo/bge-reranker-v2-m3`, which ships the weights in a co-located
        /// `model.onnx.data` blob.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        additional_files: Vec<String>,
        /// Maximum token sequence length for the tokenizer.
        ///
        /// Stored as `i64` for FFI compatibility across language bindings.
        /// Must be positive; a non-positive value is rejected with a validation error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<i64>,
        /// Scoring head for the ONNX model's output tensor.
        ///
        /// Defaults to [`RerankerHead::CrossEncoder`]. Set to
        /// [`RerankerHead::Qwen3Generative`] for Qwen3 generative-reranker
        /// checkpoints (e.g. `Qwen/Qwen3-Reranker-0.6B`).
        #[serde(default)]
        head: RerankerHead,
    },

    /// Provider-hosted reranker via liter-llm (e.g. Cohere, Jina, Voyage).
    ///
    /// The model in the nested `LlmConfig` must be a rerank-capable model ID
    /// (e.g. `"cohere/rerank-english-v3.0"`).
    Llm {
        /// LLM provider configuration specifying the model and API credentials.
        ///
        /// Boxed for the same reason as `EmbeddingModelType::Llm` -- kept in step so the two
        /// parallel enums present one shape to the generated bindings. ~keep
        llm: Box<LlmConfig>,
    },

    /// In-process reranker registered via the plugin system.
    ///
    /// The caller registers a [`crate::plugins::RerankerBackend`] once (e.g. a
    /// wrapper around a `sentence-transformers` cross-encoder or a provider client),
    /// then references it by name in config. Xberg calls back into the registered
    /// backend — no HuggingFace download, no ONNX Runtime requirement.
    ///
    /// When this variant is selected, only `max_rerank_duration_secs` applies.
    /// Model-loading fields (`batch_size`, `cache_dir`, `show_download_progress`,
    /// `acceleration`) are ignored — the host owns the model lifecycle, so there is
    /// no download to report progress for.
    ///
    /// See [`crate::plugins::register_reranker_backend`].
    Plugin {
        /// Name the backend was registered under via `register_reranker_backend`.
        name: String,
    },
}

impl Default for RerankerModelType {
    /// Returns the "balanced" preset as the default model.
    fn default() -> Self {
        Self::Preset {
            name: "balanced".to_string(),
        }
    }
}

fn default_batch_size() -> usize {
    32
}

fn default_reranker_model() -> RerankerModelType {
    RerankerModelType::Preset {
        name: "balanced".to_string(),
    }
}

fn default_max_rerank_duration_secs() -> Option<u64> {
    Some(60)
}

/// `deserialize_with` companion for `RerankerModelType` fields that may be
/// explicitly `null` in polyglot binding payloads. Treats null as the configured
/// `default_reranker_model()` (the "balanced" preset) rather than the trait
/// `Default` impl.
fn deserialize_null_model<'de, D>(deserializer: D) -> Result<RerankerModelType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<RerankerModelType>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default_reranker_model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_balanced_preset() {
        let config = RerankerConfig::default();
        assert!(matches!(
            config.model,
            RerankerModelType::Preset { ref name } if name == "balanced"
        ));
        assert_eq!(config.batch_size, 32);
        assert!(config.top_k.is_none());
        assert_eq!(config.max_rerank_duration_secs, Some(60));
    }

    #[test]
    fn default_model_type_is_balanced() {
        let model = RerankerModelType::default();
        assert!(matches!(model, RerankerModelType::Preset { ref name } if name == "balanced"));
    }

    #[test]
    fn serde_roundtrip_preset() {
        let config = RerankerConfig {
            model: RerankerModelType::Preset {
                name: "fast".to_string(),
            },
            top_k: Some(5),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: RerankerConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.model, RerankerModelType::Preset { ref name } if name == "fast"));
        assert_eq!(back.top_k, Some(5));
    }

    #[test]
    fn serde_roundtrip_custom() {
        let config = RerankerConfig {
            model: RerankerModelType::Custom {
                model_id: "cross-encoder/ms-marco-MiniLM-L6-v2".to_string(),
                model_file: None,
                additional_files: Vec::new(),
                max_length: Some(512),
                head: RerankerHead::CrossEncoder,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: RerankerConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.model,
            RerankerModelType::Custom { ref model_id, .. } if model_id.contains("ms-marco")
        ));
    }

    #[test]
    fn reranker_head_defaults_to_cross_encoder() {
        assert_eq!(RerankerHead::default(), RerankerHead::CrossEncoder);
    }

    #[test]
    fn reranker_head_serde_roundtrip() {
        for head in [RerankerHead::CrossEncoder, RerankerHead::Qwen3Generative] {
            let json = serde_json::to_string(&head).unwrap();
            let back: RerankerHead = serde_json::from_str(&json).unwrap();
            assert_eq!(back, head);
        }
        assert_eq!(
            serde_json::to_string(&RerankerHead::CrossEncoder).unwrap(),
            "\"cross_encoder\""
        );
        assert_eq!(
            serde_json::to_string(&RerankerHead::Qwen3Generative).unwrap(),
            "\"qwen3_generative\""
        );
    }

    #[test]
    fn custom_model_type_head_defaults_when_absent_from_json() {
        let json = r#"{"type": "custom", "model_id": "cross-encoder/ms-marco-MiniLM-L6-v2"}"#;
        let model: RerankerModelType = serde_json::from_str(json).unwrap();
        assert!(matches!(
            model,
            RerankerModelType::Custom {
                head: RerankerHead::CrossEncoder,
                ..
            }
        ));
    }

    #[test]
    fn null_model_field_deserializes_to_balanced() {
        let json = r#"{"model": null}"#;
        let config: RerankerConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.model, RerankerModelType::Preset { ref name } if name == "balanced"));
    }
}
