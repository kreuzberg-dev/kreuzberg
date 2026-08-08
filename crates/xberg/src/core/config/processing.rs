//! Post-processing and chunking configuration.
//!
//! Defines configuration for post-processing pipelines, text chunking,
//! and embedding generation.

use ahash::AHashSet;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Controls how markdown tables are handled when they exceed the chunk size limit.
///
/// Only applies when `chunker_type` is `Markdown`.
///
/// # Variants
///
/// * `Split` - Default behavior: tables are split at row boundaries like any
///   other block element. Continuation chunks contain only data rows without
///   the header, which can break downstream consumers that need column context.
/// * `RepeatHeader` - Prepend the table header (header row + separator row) to
///   every continuation chunk that contains data rows from the same table.
///   Adds a small amount of duplicate text but ensures each chunk is
///   self-contained for extraction, search, and LLM consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TableChunkingMode {
    /// Split tables at row boundaries (default). Continuation chunks have no header.
    #[default]
    Split,
    /// Prepend the table header to every chunk that continues a split table.
    RepeatHeader,
}

/// Type of text chunker to use.
///
/// # Variants
///
/// * `Text` - Generic text splitter, splits on whitespace and punctuation
/// * `Markdown` - Markdown-aware splitter, preserves formatting and structure
/// * `Yaml` - YAML-aware splitter, creates one chunk per top-level key
/// * `Semantic` - Topic-aware chunker. With an `EmbeddingConfig`, splits at
///   embedding-based topic shifts tuned by `topic_threshold` (default 0.75,
///   lower = more splits). Without an embedding, falls back to a
///   structural-boundary heuristic (ALL-CAPS headers, numbered sections,
///   blank-line paragraphs) and merges groups into chunks capped at
///   `max_characters` (default 1000). `topic_threshold` has no effect in the
///   fallback path. For best results, pair with an embedding model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChunkerType {
    /// Generic whitespace- and punctuation-aware text splitter (default).
    #[default]
    Text,
    /// Markdown-aware splitter that preserves heading and code-block boundaries.
    Markdown,
    /// YAML-aware splitter that creates one chunk per top-level key.
    Yaml,
    /// Topic-aware chunker that splits at embedding-based topic shifts.
    Semantic,
}

/// How chunk size is measured.
///
/// Defaults to `Characters` (Unicode character count). When using token-based sizing,
/// chunks are sized by token count according to the specified tokenizer.
///
/// Token-based sizing uses HuggingFace tokenizers loaded at runtime, or a tokenizer
/// backend you register yourself. Any tokenizer available on HuggingFace Hub can be
/// used, including OpenAI-compatible tokenizers (e.g., `Xenova/gpt-4o`,
/// `Xenova/cl100k_base`). To size chunks with your own tokenizer instead (llama.cpp/GGUF
/// vocabularies, SentencePiece models, custom vocabs), register a `TokenizerBackend`
/// with `register_tokenizer_backend` and set `model` to the registered name.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChunkSizing {
    /// Size measured in Unicode characters (default).
    #[default]
    Characters,
    /// Size measured in tokens from a HuggingFace tokenizer or a registered
    /// tokenizer backend.
    #[cfg(feature = "chunking-tokenizers")]
    Tokenizer {
        /// Name of a tokenizer backend registered via `register_tokenizer_backend`,
        /// or a HuggingFace model ID, e.g. "Xenova/gpt-4o", "bert-base-uncased".
        /// A registered backend name takes precedence over a HuggingFace ID.
        model: String,
        /// Optional cache directory override for tokenizer files.
        /// Defaults to hf-hub's standard cache (`~/.cache/huggingface/`).
        /// Can also be set via `XBERG_TOKENIZER_CACHE_DIR` environment variable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_dir: Option<std::path::PathBuf>,
    },
}

/// Post-processor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessorConfig {
    /// Enable post-processors
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Whitelist of processor names to run (None = all enabled)
    #[serde(default)]
    pub enabled_processors: Option<Vec<String>>,

    /// Blacklist of processor names to skip (None = none disabled)
    #[serde(default)]
    pub disabled_processors: Option<Vec<String>>,

    /// Pre-computed AHashSet for O(1) enabled processor lookup
    #[serde(skip)]
    pub enabled_set: Option<AHashSet<String>>,

    /// Pre-computed AHashSet for O(1) disabled processor lookup
    #[serde(skip)]
    pub disabled_set: Option<AHashSet<String>>,
}

impl PostProcessorConfig {
    /// Pre-compute HashSets for O(1) processor name lookups.
    ///
    /// This method converts the enabled/disabled processor Vec to HashSet
    /// for constant-time lookups in the pipeline.
    #[cfg(test)]
    pub(crate) fn build_lookup_sets(&mut self) {
        if let Some(ref enabled) = self.enabled_processors {
            self.enabled_set = Some(enabled.iter().cloned().collect());
        }
        if let Some(ref disabled) = self.disabled_processors {
            self.disabled_set = Some(disabled.iter().cloned().collect());
        }
    }
}

impl Default for PostProcessorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enabled_processors: None,
            disabled_processors: None,
            enabled_set: None,
            disabled_set: None,
        }
    }
}

/// Chunking configuration.
///
/// Configures text chunking for document content, including chunk size,
/// overlap, trimming behavior, and optional embeddings.
///
/// Use `..Default::default()` when constructing to allow for future field additions:
/// ```rust
/// # use xberg::ChunkingConfig;
/// let config = ChunkingConfig {
///     max_characters: 500,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    /// Maximum size per chunk (in units determined by `sizing`).
    ///
    /// When `sizing` is `Characters` (default), this is the max character count.
    /// When using token-based sizing, this is the max token count.
    ///
    /// Default: 1000
    #[serde(default = "default_chunk_size", rename = "max_chars", alias = "max_characters")]
    pub max_characters: usize,

    /// Overlap between chunks (in units determined by `sizing`).
    ///
    /// Default: 200
    #[serde(default = "default_chunk_overlap", rename = "max_overlap", alias = "overlap")]
    pub overlap: usize,

    /// Whether to trim whitespace from chunk boundaries.
    ///
    /// Default: true
    #[serde(default = "default_trim")]
    pub trim: bool,

    /// Type of chunker to use (Text or Markdown).
    ///
    /// Default: Text
    #[serde(default = "default_chunker_type")]
    pub chunker_type: ChunkerType,

    /// Optional embedding configuration for chunk embeddings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingConfig>,

    /// Optional sparse (SPLADE) embedding configuration for chunk embeddings.
    ///
    /// When set, sparse vectors are generated for each chunk's content and attached
    /// via [`crate::types::Chunk::sparse_embedding`]. Requires the `sparse-embeddings`
    /// feature; without it, a warning is emitted and no sparse vectors are attached.
    ///
    /// Config-file only: like [`super::reranker::RerankerConfig`] and the local-ONNX branch of
    /// `embedding`, this has no CLI flag and no environment variable. Only the secret/identity
    /// fields of LLM-routed configs (model, API key, base URL) get that reach. ~keep
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub sparse_embedding: Option<super::sparse_embedding::SparseEmbeddingConfig>,

    /// Optional late-interaction (ColBERT) embedding configuration for chunk embeddings.
    ///
    /// When set, multi-vector embeddings are generated for each chunk's content and
    /// attached via [`crate::types::Chunk::late_interaction`]. Requires the
    /// `late-interaction` feature; without it, a warning is emitted and no
    /// late-interaction vectors are attached.
    ///
    /// Config-file only, for the same reason as `sparse_embedding` above. ~keep
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub late_interaction: Option<super::late_interaction::LateInteractionConfig>,

    /// Use a preset configuration (overrides individual settings if provided).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,

    /// How to measure chunk size.
    ///
    /// Default: `Characters` (Unicode character count).
    /// Enable `chunking-tiktoken` or `chunking-tokenizers` features for token-based sizing.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub sizing: ChunkSizing,

    /// **Deprecated and inert** (#1393): no longer prepends anything into
    /// `content`. Setting this field has no observable effect on chunking output
    /// any more.
    ///
    /// Previously, when `true` and `chunker_type` was `Markdown`, this prepended
    /// the heading hierarchy path (e.g. `"# Title > ## Section\n\n"`) directly
    /// into each chunk's `content` string. `content` now always equals the exact
    /// `[byte_start, byte_end)` source span regardless of this flag — see
    /// [`BreadcrumbTarget`](crate::core::config::extraction::BreadcrumbTarget) for
    /// the full rationale. `heading_context`/`heading_path` on `ChunkMetadata` are
    /// populated independently of this flag, so callers lose no information —
    /// only the in-place mutation is gone.
    ///
    /// Call [`render_heading_breadcrumb`](crate::chunking::render_heading_breadcrumb)
    /// explicitly at index time instead, for the retrieval consumer that wants the
    /// breadcrumb inline.
    ///
    /// Kept only so existing callers keep compiling.
    ///
    /// Default: `false`
    #[serde(default)]
    pub prepend_heading_context: bool,

    /// Optional cosine similarity threshold for semantic topic boundary detection.
    ///
    /// Only used when `chunker_type` is `Semantic` and an `EmbeddingConfig` is
    /// provided. You almost never need to set this. When omitted, defaults to
    /// `0.75` which works well for most documents. Lower values detect more
    /// topic boundaries (more, smaller chunks); higher values detect fewer.
    /// Range: `0.0..=1.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_threshold: Option<f32>,

    /// How to handle markdown tables that exceed the chunk size limit.
    ///
    /// Only applies when `chunker_type` is `Markdown`.
    ///
    /// * `Split` (default) — tables are split at row boundaries; continuation
    ///   chunks do not repeat the header.
    /// * `RepeatHeader` — the table header row and separator are prepended to
    ///   every continuation chunk so each chunk is self-contained.
    ///
    /// Default: `Split`
    #[serde(default)]
    pub table_chunking: TableChunkingMode,

    /// **Deprecated and inert** (#1393): see
    /// [`BreadcrumbTarget`](crate::core::config::extraction::BreadcrumbTarget) for
    /// the full explanation. Neither variant has any effect on `content` any
    /// more — call
    /// [`render_heading_breadcrumb`](crate::chunking::render_heading_breadcrumb)
    /// explicitly at index time instead. Kept only for backward compatibility.
    ///
    /// Default: `Content`.
    #[serde(default)]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub breadcrumb_target: crate::core::config::extraction::BreadcrumbTarget,
}

impl ChunkingConfig {
    /// Set the cosine similarity threshold for semantic topic boundary detection.
    ///
    /// # Panics
    ///
    /// Panics if `threshold` is outside `[0.0, 1.0]`.
    #[cfg(test)]
    pub(crate) fn with_topic_threshold(mut self, threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "topic_threshold must be in [0.0, 1.0], got {threshold}"
        );
        self.topic_threshold = Some(threshold);
        self
    }

    /// Resolve a preset name into concrete chunking and embedding configuration.
    ///
    /// When `preset` is set (e.g., `"balanced"`), this overrides `max_characters` and
    /// `overlap` from the preset definition, and configures the embedding model if
    /// no embedding config was explicitly provided.
    ///
    /// If the preset name is not recognized, a warning is logged and the config
    /// is returned unchanged.
    ///
    /// Requires the `embeddings` feature. Without it, this is a no-op that returns
    /// the config unchanged.
    #[cfg(feature = "embeddings")]
    pub(crate) fn resolve_preset(&self) -> Self {
        let preset_name = match &self.preset {
            Some(name) => name,
            None => return self.clone(),
        };

        let preset = match crate::embeddings::get_preset(preset_name) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Unknown chunking preset '{}', using manual config. Available: {:?}",
                    preset_name,
                    crate::embeddings::list_presets()
                );
                return self.clone();
            }
        };

        let embedding = self.embedding.clone();

        Self {
            max_characters: preset.chunk_size,
            overlap: preset.overlap,
            embedding,
            sparse_embedding: self.sparse_embedding.clone(),
            late_interaction: self.late_interaction.clone(),
            trim: self.trim,
            chunker_type: self.chunker_type,
            preset: self.preset.clone(),
            sizing: self.sizing.clone(),
            prepend_heading_context: self.prepend_heading_context,
            topic_threshold: self.topic_threshold,
            table_chunking: self.table_chunking,
            breadcrumb_target: self.breadcrumb_target,
        }
    }

    /// Resolve a preset name (no-op without the `embeddings` feature).
    #[cfg(all(feature = "chunking", not(feature = "embeddings")))]
    pub(crate) fn resolve_preset(&self) -> Self {
        if self.preset.is_some() {
            tracing::warn!("Chunking presets require the 'embeddings' feature");
        }
        self.clone()
    }
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_characters: 1000,
            overlap: 200,
            trim: true,
            chunker_type: ChunkerType::Text,
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            preset: None,
            sizing: ChunkSizing::default(),
            prepend_heading_context: false,
            topic_threshold: None,
            table_chunking: TableChunkingMode::Split,
            breadcrumb_target: crate::core::config::extraction::BreadcrumbTarget::Content,
        }
    }
}

/// Embedding configuration for text chunks.
///
/// Configures embedding generation using ONNX models via the vendored embedding engine.
/// Requires the `embeddings` feature to be enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// The embedding model to use (defaults to "gte-modernbert-base" preset if not specified)
    #[serde(default = "default_model", deserialize_with = "deserialize_null_model")]
    pub model: EmbeddingModelType,

    /// Whether to normalize embedding vectors (recommended for cosine similarity)
    #[serde(default = "default_normalize")]
    pub normalize: bool,

    /// Batch size for embedding generation
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Show model download progress.
    ///
    /// When enabled, transfer progress for the model, tokenizer and config files is reported at
    /// `info` level on the `xberg::model_download` target while they download (#279). Covers both
    /// local backends (ONNX and static/model2vec). A warm Hugging Face cache transfers nothing and
    /// so reports nothing. Ignored by [`EmbeddingModelType::Llm`] and
    /// [`EmbeddingModelType::Plugin`], which download no model.
    #[serde(default)]
    pub show_download_progress: bool,

    /// Optional alternate Hugging Face cache root for model files.
    ///
    /// When unset, hf-hub follows `HF_HUB_CACHE`, `HUGGINGFACE_HUB_CACHE`,
    /// `HF_HOME`, XDG, and platform defaults. Prefer those environment variables
    /// when configuring the cache process-wide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,

    /// Hardware acceleration for the embedding ONNX model.
    ///
    /// When set, controls which execution provider (CPU, CUDA, CoreML, TensorRT)
    /// is used for inference. Defaults to `None` (auto-select per platform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceleration: Option<super::acceleration::AccelerationConfig>,

    /// Maximum wall-clock duration (in seconds) for a single `embed()` call when
    /// using [`EmbeddingModelType::Plugin`].
    ///
    /// Applies only to the in-process plugin path — protects against hung
    /// host-language backends (e.g. a Python callback deadlocked on the GIL,
    /// a model stuck on CUDA OOM retries, etc.). On timeout, the dispatcher
    /// returns [`crate::XbergError::Plugin`] instead of blocking forever.
    ///
    /// `None` disables the timeout. The default (60 seconds) is conservative
    /// for common in-process inference; increase for large batches on slow
    /// hardware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_embed_duration_secs: Option<u64>,

    /// Maximum number of tokens fed to the tokenizer before truncation when
    /// embedding a chunk with a local ONNX model (Preset/Custom).
    ///
    /// A chunk longer than this many tokens has its tail dropped before
    /// inference, so only the prefix contributes to the stored vector. `None`
    /// falls back to 512 (the historical default). The effective value is
    /// always capped at the model's own `model_max_length`, so raising it past
    /// what the model supports has no effect — set it to match a long-context
    /// model (e.g. 8192 for Jina/Nomic) so long chunks embed in full.
    ///
    /// Ignored by the `Llm` and `Plugin` model types, which own their own
    /// tokenization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sequence_length: Option<usize>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: EmbeddingModelType::Preset {
                name: "balanced".to_string(),
            },
            normalize: true,
            batch_size: 32,
            show_download_progress: false,
            cache_dir: None,
            acceleration: None,
            max_embed_duration_secs: Some(60),
            max_sequence_length: None,
        }
    }
}

/// Embedding model types supported by Xberg.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbeddingModelType {
    /// Use a preset model configuration (recommended)
    Preset {
        /// Preset name (e.g. "balanced", "multilingual", "large").
        name: String,
    },

    /// Use a custom ONNX model from HuggingFace
    Custom {
        /// HuggingFace model repository ID (e.g. "BAAI/bge-small-en-v1.5").
        model_id: String,
        /// Number of dimensions in the model's output embedding vectors.
        dimensions: usize,
    },

    /// Provider-hosted embedding model via liter-llm.
    ///
    /// Uses the model specified in the nested `LlmConfig` (e.g.,
    /// `"openai/text-embedding-3-small"`).
    Llm {
        /// LLM provider configuration specifying the model and API credentials.
        ///
        /// Boxed because `LlmConfig` carries liter-llm's full configuration surface and is
        /// an order of magnitude larger than the other variants, which would otherwise make
        /// every `Preset`/`Custom` value pay for it. ~keep
        llm: Box<super::llm::LlmConfig>,
    },

    /// In-process embedding backend registered via the plugin system.
    ///
    /// The caller registers an [`EmbeddingBackend`](crate::plugins::EmbeddingBackend) once
    /// (e.g. a wrapper around an already-loaded `llama-cpp-python`, `sentence-transformers`,
    /// or tuned ONNX model), then references it by name in config. Xberg calls back
    /// into the registered backend during chunking and standalone embed requests —
    /// no HuggingFace download, no ONNX Runtime requirement, no HTTP sidecar.
    ///
    /// When this variant is selected, only the following [`EmbeddingConfig`] fields
    /// apply: `normalize` (post-call L2 normalization) and `max_embed_duration_secs`
    /// (dispatcher timeout). Model-loading fields (`batch_size`, `cache_dir`,
    /// `show_download_progress`, `acceleration`) are ignored — the host owns the
    /// model lifecycle, so there is no download to report progress for.
    ///
    /// Semantic chunking falls back to [`ChunkingConfig::max_characters`] when this variant
    /// is used, since there is no preset to look a chunk-size ceiling up against — size your
    /// context window via `max_characters` directly.
    ///
    /// See [`crate::plugins::register_embedding_backend`].
    Plugin {
        /// Name the backend was registered under via `register_embedding_backend`.
        name: String,
    },
}

impl Default for EmbeddingModelType {
    /// Returns the "gte-modernbert-base" preset as the default model.
    ///
    /// The default is a valid, non-empty preset name: an empty string caused
    /// "Unknown embedding preset: " errors in every language binding that calls
    /// `EmbeddingModelType::default()` — including generated bindings that
    /// use struct-level `#[serde(default)]` instead of `default_model()`.
    /// All defaults across the codebase converge on "gte-modernbert-base"
    /// (2026-gen, 768 dims / CLS pooling — a drop-in for the prior "balanced"
    /// default, which remains available as a named preset).
    fn default() -> Self {
        Self::Preset {
            name: "gte-modernbert-base".to_string(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_chunk_size() -> usize {
    1000
}

/// Deserialize a value that may be explicitly `null` into its `Default` value.
///
/// Internally-tagged serde enums (e.g. `#[serde(tag = "type")]`) reject `null`
/// even when the containing field has `#[serde(default)]`, because that attribute
/// only covers the *missing* case. Polyglot bindings frequently emit explicit
/// `"field": null` from zero-valued mirror structs, so this helper accepts either
/// `null` or a present value and falls back to `T::default()` for null.
pub(crate) fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    let opt = Option::<T>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

fn default_chunk_overlap() -> usize {
    200
}

fn default_trim() -> bool {
    true
}

fn default_chunker_type() -> ChunkerType {
    ChunkerType::Text
}

fn default_normalize() -> bool {
    true
}

fn default_batch_size() -> usize {
    32
}

fn default_model() -> EmbeddingModelType {
    EmbeddingModelType::Preset {
        name: "gte-modernbert-base".to_string(),
    }
}

/// `deserialize_with` companion for `EmbeddingModelType` fields that may be
/// explicitly `null` in polyglot binding payloads. Treats null as the configured
/// `default_model()` (the "gte-modernbert-base" preset) rather than the trait `Default` impl
/// (which is an empty-name placeholder unsuitable for live use).
fn deserialize_null_model<'de, D>(deserializer: D) -> Result<EmbeddingModelType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<EmbeddingModelType>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(default_model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postprocessor_config_default() {
        let config = PostProcessorConfig::default();
        assert!(config.enabled);
        assert!(config.enabled_processors.is_none());
        assert!(config.disabled_processors.is_none());
    }

    #[test]
    fn test_postprocessor_config_build_lookup_sets() {
        let mut config = PostProcessorConfig {
            enabled: true,
            enabled_processors: Some(vec!["a".to_string(), "b".to_string()]),
            disabled_processors: Some(vec!["c".to_string()]),
            enabled_set: None,
            disabled_set: None,
        };

        config.build_lookup_sets();

        assert!(config.enabled_set.is_some());
        assert!(config.disabled_set.is_some());
        assert!(config.enabled_set.unwrap().contains("a"));
        assert!(config.disabled_set.unwrap().contains("c"));
    }

    #[test]
    fn test_chunking_config_defaults() {
        let config = ChunkingConfig::default();
        assert_eq!(config.max_characters, 1000);
        assert_eq!(config.overlap, 200);
        assert!(config.trim);
        assert_eq!(config.chunker_type, ChunkerType::Text);
        assert!(matches!(config.sizing, ChunkSizing::Characters));
    }

    #[test]
    fn test_embedding_config_default() {
        let config = EmbeddingConfig::default();
        assert!(config.normalize);
        assert_eq!(config.batch_size, 32);
        assert!(config.cache_dir.is_none());
    }

    /// Tests that `EmbeddingModelType::default()` returns the "gte-modernbert-base" preset.
    ///
    /// Language bindings that use struct-level `#[serde(default)]` resolve absent
    /// `model` fields via this impl. An empty-string name caused "Unknown embedding
    /// preset: " panics in `get_preset()`; the default must be a valid preset.
    #[test]
    fn test_embedding_model_type_default_is_gte_modernbert() {
        match EmbeddingModelType::default() {
            EmbeddingModelType::Preset { name } => {
                assert_eq!(
                    name, "gte-modernbert-base",
                    "Default model should be the gte-modernbert-base preset"
                );
            }
            other => panic!("Expected Preset variant, got {:?}", other),
        }
    }

    /// Tests that EmbeddingModelType::Preset serializes with "type" field (internally-tagged).
    /// This validates the API schema matches the documented format:
    /// `{"type": "preset", "name": "fast"}` NOT `{"preset": {"name": "fast"}}`
    #[test]
    fn test_embedding_model_type_preset_serialization() {
        let model = EmbeddingModelType::Preset {
            name: "fast".to_string(),
        };
        let json = serde_json::to_string(&model).unwrap();

        assert!(json.contains(r#""type":"preset""#), "Should contain type:preset field");
        assert!(json.contains(r#""name":"fast""#), "Should contain name:fast field");

        assert!(
            !json.contains(r#"{"preset":"#),
            "Should NOT use adjacently-tagged format"
        );
    }

    /// Tests that EmbeddingModelType::Preset deserializes from the documented API format.
    /// API documentation shows: `{"type": "preset", "name": "fast"}`
    #[test]
    fn test_embedding_model_type_preset_deserialization() {
        let json = r#"{"type": "preset", "name": "fast"}"#;
        let model: EmbeddingModelType = serde_json::from_str(json).unwrap();

        match model {
            EmbeddingModelType::Preset { name } => {
                assert_eq!(name, "fast");
            }
            _ => panic!("Expected Preset variant"),
        }
    }

    /// Tests that the wrong format (adjacently-tagged) is rejected.
    /// This ensures the API doesn't accept the old/wrong documentation format.
    #[test]
    fn test_embedding_model_type_rejects_wrong_format() {
        let wrong_json = r#"{"preset": {"name": "fast"}}"#;
        let result: Result<EmbeddingModelType, _> = serde_json::from_str(wrong_json);

        assert!(result.is_err(), "Should reject adjacently-tagged format");
    }

    /// Tests round-trip serialization/deserialization of EmbeddingConfig.
    #[test]
    fn test_embedding_config_roundtrip() {
        let config = EmbeddingConfig {
            model: EmbeddingModelType::Preset {
                name: "balanced".to_string(),
            },
            normalize: true,
            batch_size: 64,
            show_download_progress: false,
            cache_dir: None,
            acceleration: None,
            max_embed_duration_secs: Some(60),
            max_sequence_length: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EmbeddingConfig = serde_json::from_str(&json).unwrap();

        match deserialized.model {
            EmbeddingModelType::Preset { name } => {
                assert_eq!(name, "balanced");
            }
            _ => panic!("Expected Preset variant"),
        }
        assert!(deserialized.normalize);
        assert_eq!(deserialized.batch_size, 64);
    }

    /// Tests Custom model type serialization format.
    #[test]
    fn test_embedding_model_type_custom_serialization() {
        let model = EmbeddingModelType::Custom {
            model_id: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            dimensions: 384,
        };
        let json = serde_json::to_string(&model).unwrap();

        assert!(json.contains(r#""type":"custom""#), "Should contain type:custom field");
        assert!(json.contains(r#""model_id":"#), "Should contain model_id field");
        assert!(json.contains(r#""dimensions":384"#), "Should contain dimensions field");
    }

    #[test]
    #[cfg(feature = "embeddings")]
    fn test_resolve_preset_balanced() {
        let config = ChunkingConfig {
            preset: Some("balanced".to_string()),
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert_eq!(resolved.max_characters, 1024);
        assert_eq!(resolved.overlap, 100);
        assert!(resolved.embedding.is_none());
    }

    #[test]
    #[cfg(feature = "embeddings")]
    fn test_resolve_preset_preserves_explicit_embedding() {
        let explicit_embedding = EmbeddingConfig {
            model: EmbeddingModelType::Custom {
                model_id: "custom/model".to_string(),
                dimensions: 512,
            },
            batch_size: 64,
            ..Default::default()
        };
        let config = ChunkingConfig {
            preset: Some("fast".to_string()),
            embedding: Some(explicit_embedding),
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert_eq!(resolved.max_characters, 512);
        assert_eq!(resolved.overlap, 50);
        match &resolved.embedding.unwrap().model {
            EmbeddingModelType::Custom { model_id, .. } => assert_eq!(model_id, "custom/model"),
            _ => panic!("Expected Custom model type to be preserved"),
        }
    }

    #[cfg(any(feature = "embeddings", feature = "chunking"))]
    #[test]
    fn test_resolve_preset_no_preset_returns_unchanged() {
        let config = ChunkingConfig {
            max_characters: 500,
            overlap: 50,
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert_eq!(resolved.max_characters, 500);
        assert_eq!(resolved.overlap, 50);
        assert!(resolved.embedding.is_none());
    }

    #[cfg(any(feature = "embeddings", feature = "chunking"))]
    #[test]
    fn test_resolve_preset_unknown_name_returns_unchanged() {
        let config = ChunkingConfig {
            max_characters: 500,
            preset: Some("nonexistent".to_string()),
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert_eq!(resolved.max_characters, 500);
    }

    #[test]
    fn test_embedding_model_type_llm_roundtrip() {
        let model_type = EmbeddingModelType::Llm {
            llm: Box::new(crate::core::config::llm::LlmConfig {
                model: "openai/text-embedding-3-small".to_string(),
                ..Default::default()
            }),
        };
        let json = serde_json::to_string(&model_type).unwrap();
        assert!(json.contains("\"type\":\"llm\""));
        assert!(json.contains("openai/text-embedding-3-small"));

        let deserialized: EmbeddingModelType = serde_json::from_str(&json).unwrap();
        match deserialized {
            EmbeddingModelType::Llm { llm } => {
                assert_eq!(llm.model, "openai/text-embedding-3-small");
            }
            _ => panic!("Expected Llm variant"),
        }
    }

    #[test]
    #[should_panic(expected = "topic_threshold must be in [0.0, 1.0]")]
    fn test_with_topic_threshold_panics_above_one() {
        ChunkingConfig::default().with_topic_threshold(1.1);
    }

    #[test]
    #[should_panic(expected = "topic_threshold must be in [0.0, 1.0]")]
    fn test_with_topic_threshold_panics_below_zero() {
        ChunkingConfig::default().with_topic_threshold(-0.1);
    }

    #[test]
    fn test_with_topic_threshold_accepts_boundary_values() {
        let config = ChunkingConfig::default().with_topic_threshold(0.0);
        assert_eq!(config.topic_threshold, Some(0.0));

        let config = ChunkingConfig::default().with_topic_threshold(1.0);
        assert_eq!(config.topic_threshold, Some(1.0));
    }

    /// Tests Custom model type deserialization.
    #[test]
    fn test_embedding_model_type_custom_deserialization() {
        let json = r#"{"type": "custom", "model_id": "test/model", "dimensions": 512}"#;
        let model: EmbeddingModelType = serde_json::from_str(json).unwrap();

        match model {
            EmbeddingModelType::Custom { model_id, dimensions } => {
                assert_eq!(model_id, "test/model");
                assert_eq!(dimensions, 512);
            }
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_embedding_model_type_plugin_roundtrip() {
        let model = EmbeddingModelType::Plugin {
            name: "lilbee-llamacpp".to_string(),
        };
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("\"type\":\"plugin\""));
        assert!(json.contains("lilbee-llamacpp"));

        let deserialized: EmbeddingModelType = serde_json::from_str(&json).unwrap();
        match deserialized {
            EmbeddingModelType::Plugin { name } => assert_eq!(name, "lilbee-llamacpp"),
            _ => panic!("Expected Plugin variant"),
        }
    }

    #[test]
    fn test_embedding_model_type_plugin_deserialization() {
        let json = r#"{"type": "plugin", "name": "my-embedder"}"#;
        let model: EmbeddingModelType = serde_json::from_str(json).unwrap();
        match model {
            EmbeddingModelType::Plugin { name } => assert_eq!(name, "my-embedder"),
            _ => panic!("Expected Plugin variant"),
        }
    }

    /// Preset with no explicit embedding: embedding must remain None.
    ///
    /// Before the fix, `resolve_preset()` would silently inject an
    /// `EmbeddingConfig` whenever a preset was configured, causing every
    /// chunk to have an unexpected `.embedding` field populated.
    #[test]
    #[cfg(feature = "embeddings")]
    fn test_resolve_preset_does_not_inject_embedding_when_none() {
        let config = ChunkingConfig {
            preset: Some("multilingual".to_string()),
            embedding: None,
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert!(
            resolved.embedding.is_none(),
            "preset alone must not inject an EmbeddingConfig (#797)"
        );
    }

    /// Preset with an explicit embedding: the embedding must be preserved unchanged.
    #[test]
    #[cfg(feature = "embeddings")]
    fn test_resolve_preset_preserves_explicit_embedding_config() {
        let explicit = EmbeddingConfig {
            model: EmbeddingModelType::Custom {
                model_id: "my-org/model".to_string(),
                dimensions: 768,
            },
            batch_size: 16,
            ..Default::default()
        };
        let config = ChunkingConfig {
            preset: Some("multilingual".to_string()),
            embedding: Some(explicit),
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        let emb = resolved
            .embedding
            .expect("explicit embedding must survive resolve_preset");
        assert_eq!(emb.batch_size, 16);
        match emb.model {
            EmbeddingModelType::Custom { model_id, dimensions } => {
                assert_eq!(model_id, "my-org/model");
                assert_eq!(dimensions, 768);
            }
            other => panic!("expected Custom model type, got {other:?}"),
        }
    }

    /// No preset, no embedding: embedding must stay None (regression guard).
    #[cfg(any(feature = "embeddings", feature = "chunking"))]
    #[test]
    fn test_resolve_preset_no_preset_no_embedding_stays_none() {
        let config = ChunkingConfig {
            preset: None,
            embedding: None,
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert!(resolved.embedding.is_none(), "no-preset path must not touch embedding");
    }

    #[test]
    fn table_chunking_mode_defaults_to_split_when_field_absent() {
        let c: ChunkingConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c.table_chunking, TableChunkingMode::Split);
    }

    /// Regression guard for #268: a `ChunkingConfig` JSON payload written before
    /// `sparse_embedding`/`late_interaction` existed (i.e. missing both keys) must still
    /// deserialize, with both fields defaulting to `None`. `ChunkingConfig` is nested inside
    /// `ExtractionConfig`, which is `#[serde(deny_unknown_fields)]` — this guards the other
    /// direction of that contract: old payloads must not become invalid just because the
    /// schema grew new optional fields.
    #[test]
    fn sparse_and_late_interaction_configs_default_to_none_when_absent_from_json() {
        let c: ChunkingConfig = serde_json::from_str("{}").unwrap();
        assert!(c.sparse_embedding.is_none());
        assert!(c.late_interaction.is_none());
    }

    /// `ChunkingConfig::default()` must leave both new vector configs unset (#268) — no
    /// behaviour change for existing callers who never opt in.
    #[test]
    fn chunking_config_default_has_no_sparse_or_late_interaction_config() {
        let config = ChunkingConfig::default();
        assert!(config.sparse_embedding.is_none());
        assert!(config.late_interaction.is_none());
    }

    /// `resolve_preset()` must carry an explicitly-set `sparse_embedding`/`late_interaction`
    /// config through unchanged (#268), the same way it already preserves `embedding`.
    #[cfg(any(feature = "embeddings", feature = "chunking"))]
    #[test]
    fn resolve_preset_preserves_explicit_sparse_and_late_interaction_configs() {
        let config = ChunkingConfig {
            preset: Some("balanced".to_string()),
            sparse_embedding: Some(crate::core::config::SparseEmbeddingConfig {
                batch_size: 4,
                ..Default::default()
            }),
            late_interaction: Some(crate::core::config::LateInteractionConfig {
                batch_size: 8,
                ..Default::default()
            }),
            ..Default::default()
        };
        let resolved = config.resolve_preset();
        assert_eq!(
            resolved
                .sparse_embedding
                .expect("sparse_embedding must survive resolve_preset")
                .batch_size,
            4
        );
        assert_eq!(
            resolved
                .late_interaction
                .expect("late_interaction must survive resolve_preset")
                .batch_size,
            8
        );
    }
}
