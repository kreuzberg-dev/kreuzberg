//! Sparse (SPLADE) embedding configuration types.
//!
//! Configuration for sparse learned embeddings produced by a `BertForMaskedLM`
//! ONNX model (SPLADE). Unlike dense embeddings, each output is a high-dimensional
//! but mostly-zero vocabulary vector, stored as parallel `(indices, values)`
//! arrays. These unlock hybrid dense+sparse retrieval.
//!
//! Since v5.0.0.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the sparse-embedding pipeline.
///
/// Controls which model to use, batching, and download/cache behavior for the
/// local ONNX SPLADE model.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseEmbeddingConfig {
    /// The sparse-embedding model to use (defaults to the "opensearch-v3-distill" preset).
    #[serde(default = "default_sparse_model", deserialize_with = "deserialize_null_model")]
    pub model: SparseEmbeddingModelType,

    /// Batch size for local ONNX inference.
    ///
    /// SPLADE emits a `[seq, vocab]` logit tensor per document, so memory scales
    /// with batch size — keep this modest.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Maximum token sequence length for the tokenizer.
    #[serde(default = "default_max_length")]
    pub max_length: usize,

    /// Show model download progress (local ONNX path only).
    ///
    /// When enabled, transfer progress for the model, tokenizer and config files is reported at
    /// `info` level on the `xberg::model_download` target while they download (#279). A warm
    /// Hugging Face cache transfers nothing and so reports nothing. Ignored by
    /// [`SparseEmbeddingModelType::Plugin`], which downloads no model.
    #[serde(default)]
    pub show_download_progress: bool,

    /// Optional alternate Hugging Face cache root for model files.
    ///
    /// When unset, hf-hub follows the standard Hugging Face environment and
    /// platform cache conventions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Hardware acceleration for the sparse-embedding ONNX model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Maximum wall-clock duration (in seconds) for a single embed call when
    /// using [`SparseEmbeddingModelType::Plugin`]. `None` disables the timeout.
    #[serde(default = "default_max_embed_duration_secs", skip_serializing_if = "Option::is_none")]
    pub max_embed_duration_secs: Option<u64>,
}

impl Default for SparseEmbeddingConfig {
    fn default() -> Self {
        Self {
            model: default_sparse_model(),
            batch_size: default_batch_size(),
            max_length: default_max_length(),
            show_download_progress: false,
            cache_dir: None,
            acceleration: None,
            max_embed_duration_secs: default_max_embed_duration_secs(),
        }
    }
}

/// Sparse-embedding model types supported by Xberg.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SparseEmbeddingModelType {
    /// Use a preset SPLADE model (recommended).
    Preset {
        /// Preset name (e.g. "splade").
        name: String,
    },

    /// Use a custom SPLADE (`BertForMaskedLM`) ONNX model from HuggingFace.
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

    /// In-process sparse-embedding backend registered via the plugin system.
    Plugin {
        /// Name the backend was registered under.
        name: String,
    },
}

impl Default for SparseEmbeddingModelType {
    fn default() -> Self {
        Self::Preset {
            name: "opensearch-v3-distill".to_string(),
        }
    }
}

fn default_sparse_model() -> SparseEmbeddingModelType {
    SparseEmbeddingModelType::default()
}

fn default_batch_size() -> usize {
    16
}

fn default_max_length() -> usize {
    256
}

fn default_max_embed_duration_secs() -> Option<u64> {
    Some(60)
}

/// Accept an explicit `null` model field and fall back to the default, mirroring
/// the dense-embedding and reranker configs' handling of zero-valued binding mirrors.
fn deserialize_null_model<'de, D>(deserializer: D) -> Result<SparseEmbeddingModelType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<SparseEmbeddingModelType>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_opensearch_preset() {
        let config = SparseEmbeddingConfig::default();
        assert!(matches!(config.model, SparseEmbeddingModelType::Preset { name } if name == "opensearch-v3-distill"));
        assert_eq!(config.batch_size, 16);
        assert_eq!(config.max_length, 256);
    }

    #[test]
    fn null_model_deserializes_to_default() {
        let json = r#"{"model": null}"#;
        let config: SparseEmbeddingConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.model, SparseEmbeddingModelType::Preset { name } if name == "opensearch-v3-distill"));
    }

    #[test]
    fn custom_model_roundtrips() {
        let config = SparseEmbeddingConfig {
            model: SparseEmbeddingModelType::Custom {
                model_id: "org/splade".to_string(),
                model_file: Some("onnx/model.onnx".to_string()),
                additional_files: vec![],
                max_length: Some(256),
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: SparseEmbeddingConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.model, SparseEmbeddingModelType::Custom { model_id, .. } if model_id == "org/splade"));
    }
}
