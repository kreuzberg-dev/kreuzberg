//! ColBERT late-interaction (multi-vector) configuration types.
//!
//! Configuration for multi-vector embeddings produced by a ColBERT-style ONNX
//! model. Unlike dense or sparse embeddings, each output is a *sequence* of
//! per-token vectors (one per input token, including the ColBERT `[Q]`/`[D]`
//! marker) rather than a single pooled vector. Retrieval scores documents
//! against a query via MaxSim (see [`crate::late_interaction::max_sim_score`])
//! instead of a single dot product.
//!
//! Since v5.0.0.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the late-interaction (ColBERT) pipeline.
///
/// Controls which model to use, batching, and download/cache behavior for the
/// local ONNX ColBERT model.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LateInteractionConfig {
    /// The late-interaction model to use (defaults to the "gte-moderncolbert" preset).
    #[serde(
        default = "default_late_interaction_model",
        deserialize_with = "deserialize_null_model"
    )]
    pub model: LateInteractionModelType,

    /// Batch size for local ONNX inference.
    ///
    /// ColBERT emits a `[seq, dim]` multi-vector embedding per document, so
    /// memory scales with batch size — keep this modest.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Maximum token sequence length for the tokenizer (documents).
    #[serde(default = "default_max_length")]
    pub max_length: usize,

    /// Fixed padded length for query augmentation.
    ///
    /// ColBERT queries are padded (with the mask token, kept attention-live)
    /// to exactly this many tokens rather than truncated/left as-is — this is
    /// the "query augmentation" trick from the ColBERT paper.
    #[serde(default = "default_query_max_length")]
    pub query_max_length: usize,

    /// Show model download progress (local ONNX path only).
    ///
    /// When enabled, transfer progress for the model, tokenizer and config files is reported at
    /// `info` level on the `xberg::model_download` target while they download (#279). A warm
    /// Hugging Face cache transfers nothing and so reports nothing. Ignored by
    /// [`LateInteractionModelType::Plugin`], which downloads no model.
    #[serde(default)]
    pub show_download_progress: bool,

    /// Optional alternate Hugging Face cache root for model files.
    ///
    /// When unset, hf-hub follows the standard Hugging Face environment and
    /// platform cache conventions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Hardware acceleration for the late-interaction ONNX model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Maximum wall-clock duration (in seconds) for a single embed call when
    /// using [`LateInteractionModelType::Plugin`]. `None` disables the timeout.
    #[serde(default = "default_max_embed_duration_secs", skip_serializing_if = "Option::is_none")]
    pub max_embed_duration_secs: Option<u64>,
}

impl Default for LateInteractionConfig {
    fn default() -> Self {
        Self {
            model: default_late_interaction_model(),
            batch_size: default_batch_size(),
            max_length: default_max_length(),
            query_max_length: default_query_max_length(),
            show_download_progress: false,
            cache_dir: None,
            acceleration: None,
            max_embed_duration_secs: default_max_embed_duration_secs(),
        }
    }
}

/// Late-interaction model types supported by Xberg.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LateInteractionModelType {
    /// Use a preset ColBERT model (recommended).
    Preset {
        /// Preset name (e.g. "colbert").
        name: String,
    },

    /// Use a custom ColBERT ONNX model from HuggingFace.
    Custom {
        /// HuggingFace model repository ID.
        model_id: String,
        /// Path to the ONNX file within the repo. Defaults to `"onnx/model.onnx"`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_file: Option<String>,
        /// Sibling files that must be downloaded alongside `model_file`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        additional_files: Vec<String>,
        /// Maximum token sequence length. Stored as `i64` for FFI compatibility;
        /// negative values are clamped to the model default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<i64>,
    },

    /// In-process late-interaction backend registered via the plugin system.
    Plugin {
        /// Name the backend was registered under.
        name: String,
    },
}

impl Default for LateInteractionModelType {
    fn default() -> Self {
        Self::Preset {
            name: "gte-moderncolbert".to_string(),
        }
    }
}

fn default_late_interaction_model() -> LateInteractionModelType {
    LateInteractionModelType::default()
}

fn default_batch_size() -> usize {
    16
}

fn default_max_length() -> usize {
    512
}

fn default_query_max_length() -> usize {
    32
}

fn default_max_embed_duration_secs() -> Option<u64> {
    Some(60)
}

/// Accept an explicit `null` model field and fall back to the default, mirroring
/// the dense-embedding, reranker, and sparse-embedding configs' handling of
/// zero-valued binding mirrors.
fn deserialize_null_model<'de, D>(deserializer: D) -> Result<LateInteractionModelType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<LateInteractionModelType>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_gte_moderncolbert_preset() {
        let config = LateInteractionConfig::default();
        assert!(matches!(config.model, LateInteractionModelType::Preset { name } if name == "gte-moderncolbert"));
        assert_eq!(config.batch_size, 16);
        assert_eq!(config.max_length, 512);
        assert_eq!(config.query_max_length, 32);
    }

    #[test]
    fn null_model_deserializes_to_default() {
        let json = r#"{"model": null}"#;
        let config: LateInteractionConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.model, LateInteractionModelType::Preset { name } if name == "gte-moderncolbert"));
    }

    #[test]
    fn custom_model_roundtrips() {
        let config = LateInteractionConfig {
            model: LateInteractionModelType::Custom {
                model_id: "org/colbert".to_string(),
                model_file: Some("onnx/model.onnx".to_string()),
                additional_files: vec![],
                max_length: Some(512),
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: LateInteractionConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.model, LateInteractionModelType::Custom { model_id, .. } if model_id == "org/colbert"));
    }
}
