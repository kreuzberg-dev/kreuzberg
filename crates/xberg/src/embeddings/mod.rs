//! Embedding generation support for RAG (Retrieval-Augmented Generation) systems.
//!
//! This module provides text embedding generation using ONNX models via a vendored
//! text embedding inference engine. Embeddings can be generated for text chunks to
//! enable semantic search and RAG pipelines.
//!
//! # Features
//!
//! - Multiple pre-configured models optimized for different use cases
//! - Preset configurations for common RAG scenarios
//! - Full customization of model location and parameters
//! - Batch processing for efficient embedding generation
//! - Thread-safe inference without mutex contention
//! - Optional GPU acceleration via ONNX Runtime execution providers
//!
//! # ONNX Runtime Requirement
//!
//! **CRITICAL**: This module requires ONNX Runtime to be installed on the system.
//! The `embeddings` feature uses dynamic loading (`ort-load-dynamic`), which detects
//! the ONNX Runtime library at runtime.
//!
//! ## Installation Instructions
//!
//! - **macOS**: `brew install onnxruntime`
//! - **Linux (Ubuntu/Debian)**: `apt install libonnxruntime libonnxruntime-dev`
//! - **Linux (Fedora)**: `dnf install onnxruntime onnxruntime-devel`
//! - **Linux (Arch)**: `pacman -S onnxruntime`
//! - **Windows (MSVC)**: Download from <https://github.com/microsoft/onnxruntime/releases> and add to PATH
//!
//! Alternatively, set the `ORT_DYLIB_PATH` environment variable to the ONNX Runtime library path.
//!
//! For Docker/containers, install via package manager in your base image.
//! Verified packages: Ubuntu 22.04+, Fedora 38+, Arch Linux.
//!
//! ## Platform Limitations
//!
//! **Windows MinGW builds are not supported**. ONNX Runtime requires the MSVC toolchain on Windows.
//! Please use Windows MSVC builds or disable the embeddings feature.
//!
//! # Static (model2vec) Embeddings
//!
//! The `"lightweight"` preset (and any future `Static`-backend preset) runs
//! through a pure-Rust model2vec engine instead of ONNX Runtime, gated behind
//! the `static-embeddings` feature. It requires no native ONNX dependency and is
//! the only dense-embedding backend available on `no-ort-target` (WASM, Android
//! x86_64 emulator). Select it the same way as any other preset:
//! `EmbeddingConfig { model: EmbeddingModelType::Preset { name: "lightweight".into() }, .. }`.
//!
//! # Example
//!
//! ```rust,ignore
//! use xberg::{extract, ChunkingConfig, EmbeddingConfig, ExtractInput, ExtractionConfig};
//!
//! let config = ExtractionConfig {
//!     chunking: Some(ChunkingConfig {
//!         preset: Some("balanced".to_string()),
//!         embedding: Some(EmbeddingConfig::default()),
//!         ..Default::default()
//!     }),
//!     ..Default::default()
//! };
//!
//! let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
//! let result = output.results.into_iter().next().expect("one input yields one result");
//! for chunk in result.chunks.unwrap() {
//!     if let Some(embedding) = chunk.embedding {
//!         println!("Chunk has {} dimension embedding", embedding.len());
//!     }
//! }
//! ```

#[cfg(feature = "embeddings")]
/// Core ONNX embedding inference engine with thread-safe concurrent inference.
pub mod engine;

/// Pure-Rust static (model2vec) embedding inference engine — no ONNX Runtime.
/// The only dense-embedding backend available on `no-ort-target` (WASM/Android).
#[cfg(feature = "static-embeddings")]
pub mod static_engine;

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[cfg(feature = "embeddings")]
use ahash::AHashMap;
#[cfg(feature = "embeddings")]
use engine::EmbeddingEngine;
#[cfg(any(
    feature = "embeddings",
    all(feature = "static-embeddings", feature = "tokio-runtime")
))]
use std::sync::Arc;
#[cfg(feature = "embeddings")]
use std::sync::RwLock;

#[cfg(feature = "embeddings")]
type CachedEngine = Arc<EmbeddingEngine>;

#[cfg(feature = "embeddings")]
static ENGINE_CACHE: LazyLock<RwLock<AHashMap<String, CachedEngine>>> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Global semaphore that limits concurrent ONNX embedding inference calls.
///
/// Prevents resource exhaustion when many async callers invoke `embed_texts_async`
/// against the local (Preset/Custom, ONNX or static) path simultaneously. The Llm
/// and Plugin variants short-circuit out of `embed_texts_async` before reaching
/// the semaphore — they don't share the local-inference resource pool. The permit
/// count is set once on first access using the thread budget, matching the pattern
/// used elsewhere (e.g., image OCR, batch extraction).
#[cfg(all(
    any(feature = "embeddings", feature = "static-embeddings"),
    feature = "tokio-runtime"
))]
static EMBED_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    let budget = crate::core::config::concurrency::resolve_thread_budget(None);
    Arc::new(tokio::sync::Semaphore::new(budget))
});

/// Inference backend that an [`EmbeddingPreset`] runs on.
///
/// `Onnx` presets require the `embeddings` feature (ONNX Runtime, not available on
/// WASM/Android x86_64 emulator). `Static` presets require `static-embeddings`
/// (pure-Rust model2vec inference, no ORT — the only dense-embedding backend
/// available on `no-ort-target`).
///
/// Defaults to `Onnx` via `#[serde(default)]` so every existing preset payload
/// (which predates this field) keeps deserializing without change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingBackend {
    /// ONNX Runtime transformer inference (the historical, default backend).
    #[default]
    Onnx,
    /// Pure-Rust static (model2vec) inference — no ONNX Runtime.
    Static,
}

/// Preset configurations for common RAG use cases.
///
/// Each preset combines chunk size, overlap, and embedding model
/// to provide an optimized configuration for specific scenarios.
///
/// All string fields are owned `String` for FFI compatibility — instances
/// are safe to clone and pass across language boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingPreset {
    /// Short identifier for this preset (e.g. `"balanced"`, `"fast"`, `"quality"`).
    pub name: String,
    /// Target chunk size in characters.
    pub chunk_size: usize,
    /// Overlap between consecutive chunks in characters.
    pub overlap: usize,
    /// HuggingFace repository name for the model.
    pub model_repo: String,
    /// Pooling strategy: "cls" or "mean". Static (model2vec) presets always mean-pool.
    pub pooling: String,
    /// Path to the model file within the repo (ONNX file for `Onnx`, model2vec
    /// `model.safetensors` for `Static`).
    pub model_file: String,
    /// Embedding vector dimension produced by this model.
    pub dimensions: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
    /// Which inference backend this preset runs on. Defaults to `Onnx` for
    /// back-compat with presets/payloads that predate this field.
    #[serde(default)]
    pub backend: EmbeddingBackend,
    /// Sibling files downloaded alongside `model_file`. Large fp32 ONNX exports
    /// (Qwen3-Embedding, Arctic-Embed-v2.0) store weights in an external-data
    /// `model.onnx.data` blob that ORT loads by relative path; single-file
    /// models leave this empty.
    #[serde(default)]
    pub additional_files: Vec<String>,
    /// Instruction prefix prepended to *query-side* text before encoding.
    /// Asymmetric retrieval models (Arctic-Embed, E5) are trained with a
    /// `"query: "`-style prefix on queries only; document text is never
    /// prefixed. `None` for symmetric models.
    #[serde(default)]
    pub query_prefix: Option<String>,
}

/// All available embedding presets.
/// SHA-256 manifest pinning every hosted embedding preset file, verified at
/// download time by [`crate::onnx::download_model_files`].
#[cfg(any(
    feature = "embeddings",
    all(feature = "static-embeddings", not(target_arch = "wasm32")),
    test
))]
pub(crate) const EMBEDDING_SHA256_MANIFEST: &str = include_str!("presets.sha256sum");

#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
pub(crate) const EMBEDDING_MODEL_REVISION: &str = "4b127809f88a5aa1569d1238032b5ff40e5879bc";

pub static EMBEDDING_PRESETS: LazyLock<Vec<EmbeddingPreset>> = LazyLock::new(|| {
    vec![
        EmbeddingPreset {
            name: "fast".to_string(),
            chunk_size: 512,
            overlap: 50,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "mean".to_string(),
            model_file: "all-MiniLM-L6-v2/model_quantized.onnx".to_string(),
            dimensions: 384,
            description: "Fast embedding with quantized model (384 dims, ~22M params). Best for: Quick prototyping, development, resource-constrained environments.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "balanced".to_string(),
            chunk_size: 1024,
            overlap: 100,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "cls".to_string(),
            model_file: "bge-base-en-v1.5/model.onnx".to_string(),
            dimensions: 768,
            description: "Balanced quality and speed (768 dims, ~109M params). Best for: General-purpose RAG, production deployments, English documents.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "quality".to_string(),
            chunk_size: 2000,
            overlap: 200,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "cls".to_string(),
            model_file: "bge-large-en-v1.5/model.onnx".to_string(),
            dimensions: 1024,
            description: "High quality with larger context (1024 dims, ~335M params). Best for: Complex documents, maximum accuracy, sufficient compute resources.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "multilingual".to_string(),
            chunk_size: 1024,
            overlap: 100,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "mean".to_string(),
            model_file: "multilingual-e5-base/model.onnx".to_string(),
            dimensions: 768,
            description: "Multilingual support (768 dims, 100+ languages). Best for: International documents, mixed-language content, global applications.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "gte-modernbert-base".to_string(),
            chunk_size: 1024,
            overlap: 100,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "cls".to_string(),
            model_file: "gte-modernbert-base/model.onnx".to_string(),
            dimensions: 768,
            description: "GTE ModernBERT base (768 dims, 2026-gen, 8192 context). Best for: general-purpose English RAG with long-context ModernBERT tokenization.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "lightweight".to_string(),
            chunk_size: 512,
            overlap: 50,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "mean".to_string(),
            model_file: "potion-base-8m/model.safetensors".to_string(),
            dimensions: 256,
            description: "Static (model2vec) embedding — pure Rust, no ONNX Runtime (256 dims, ~7.5M params). Best for: WASM, Android, and other no-ORT targets; extremely fast CPU-only inference.".to_string(),
            backend: EmbeddingBackend::Static,
            additional_files: Vec::new(),
            query_prefix: None,
        },
        EmbeddingPreset {
            name: "arctic-embed-m-v2.0".to_string(),
            chunk_size: 1024,
            overlap: 100,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "cls".to_string(),
            model_file: "arctic-embed-m-v2.0/model.onnx".to_string(),
            dimensions: 768,
            description: "Snowflake Arctic-Embed-M v2.0 (768 dims, multilingual, 2026-gen). Asymmetric retrieval: queries are prefixed with \"query: \". Best for: multilingual RAG where query/document roles are known.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: vec!["arctic-embed-m-v2.0/model.onnx.data".to_string()],
            query_prefix: Some("query: ".to_string()),
        },
        EmbeddingPreset {
            name: "qwen3-embedding-0.6b".to_string(),
            chunk_size: 2000,
            overlap: 200,
            model_repo: "xberg-io/embedding-models".to_string(),
            pooling: "last".to_string(),
            model_file: "qwen3-embedding-0.6b/model.onnx".to_string(),
            dimensions: 1024,
            description: "Qwen3-Embedding 0.6B (1024 dims, decoder-style last-token pooling, 32k context, multilingual, 2026-gen). Best for: highest-quality multilingual/long-context retrieval when compute allows.".to_string(),
            backend: EmbeddingBackend::Onnx,
            additional_files: vec!["qwen3-embedding-0.6b/model.onnx.data".to_string()],
            query_prefix: None,
        },
    ]
});

/// Get a preset by name (returns an owned clone for FFI compatibility).
pub(crate) fn get_preset(name: &str) -> Option<EmbeddingPreset> {
    EMBEDDING_PRESETS.iter().find(|p| p.name == name).cloned()
}

/// Query-side instruction prefix for the given embedding config, if the
/// resolved preset defines one.
///
/// Asymmetric retrieval models (e.g. Arctic-Embed) are trained with a
/// `"query: "`-style prefix on the query only — the RAG query path prepends
/// this before embedding, while document text is embedded verbatim. Returns
/// `None` for symmetric presets, custom models, and non-preset backends.
#[cfg_attr(alef, alef(skip))]
pub fn embedding_query_prefix(config: &crate::core::config::EmbeddingConfig) -> Option<String> {
    match &config.model {
        crate::core::config::EmbeddingModelType::Preset { name } => get_preset(name).and_then(|p| p.query_prefix),
        _ => None,
    }
}

/// Get the chunk_size for a preset by name.
#[cfg(feature = "embeddings")]
pub(crate) fn preset_chunk_size(name: &str) -> Option<usize> {
    get_preset(name).map(|p| p.chunk_size)
}

/// List all available preset names (owned clones for FFI compatibility).
pub(crate) fn list_presets() -> Vec<String> {
    EMBEDDING_PRESETS.iter().map(|p| p.name.clone()).collect()
}

/// Module-tagged error constructor threaded into the shared onnx helpers.
#[cfg(feature = "embeddings")]
fn embed_err(msg: String) -> crate::XbergError {
    crate::XbergError::embedding(msg)
}

/// Default tokenizer truncation length when `EmbeddingConfig.max_sequence_length`
/// is unset. Matches the historical hardcoded value; the effective length is still
/// capped at the model's own `model_max_length` in [`load_tokenizer`].
#[cfg(feature = "embeddings")]
const DEFAULT_EMBEDDING_MAX_SEQUENCE_LENGTH: usize = 512;

/// Resolve model info (repo, model file, pooling) from an EmbeddingModelType config.
///
/// Only handles the ONNX path — callers must reject `Static`-backend presets
/// before calling this (see [`embed_texts`]'s dispatch, which branches on
/// `preset.backend` first).
#[cfg(feature = "embeddings")]
fn resolve_model_info(
    model_type: &crate::core::config::EmbeddingModelType,
) -> crate::Result<(String, String, Vec<String>, engine::Pooling)> {
    match model_type {
        crate::core::config::EmbeddingModelType::Preset { name } => {
            let preset = get_preset(name)
                .ok_or_else(|| crate::XbergError::embedding(format!("Unknown embedding preset: {name}")))?;
            if preset.backend == EmbeddingBackend::Static {
                return Err(crate::XbergError::embedding(format!(
                    "Preset '{name}' uses the static (model2vec) backend, which has no ONNX model to warm or download. Rebuild with --features static-embeddings and call embed_texts directly."
                )));
            }
            let pooling = match preset.pooling.as_str() {
                "cls" => engine::Pooling::Cls,
                "last" => engine::Pooling::Last,
                _ => engine::Pooling::Mean,
            };
            Ok((preset.model_repo, preset.model_file, preset.additional_files, pooling))
        }
        crate::core::config::EmbeddingModelType::Custom { model_id, .. } => Ok((
            model_id.clone(),
            "onnx/model.onnx".to_string(),
            Vec::new(),
            engine::Pooling::Mean,
        )),
        crate::core::config::EmbeddingModelType::Llm { .. } => Err(crate::XbergError::embedding(
            "LLM embeddings have no local model to warm or download — the provider serves them over HTTP at embed time.",
        )),
        crate::core::config::EmbeddingModelType::Plugin { .. } => Err(crate::XbergError::embedding(
            "Plugin embeddings have no local model to warm or download — the registered backend owns the model lifecycle.",
        )),
    }
}

/// Get or initialize an embedding engine from cache.
///
/// Downloads model files from HuggingFace if needed, loads the tokenizer,
/// creates an ORT session, and caches the engine for reuse.
#[cfg(feature = "embeddings")]
#[allow(clippy::too_many_arguments)]
fn get_or_init_engine(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    pooling: engine::Pooling,
    max_sequence_length: usize,
    cache_dir: Option<std::path::PathBuf>,
    progress: crate::core::config::DownloadProgress,
    accel: Option<crate::core::config::acceleration::AccelerationConfig>,
) -> crate::Result<Arc<EmbeddingEngine>> {
    let revision = (repo_name == "xberg-io/embedding-models").then_some(EMBEDDING_MODEL_REVISION);
    let cache_key = crate::model_download::hf_cache_key(cache_dir.as_deref());
    let engine_key = format!(
        "{repo_name}_{model_file}_{}_{max_sequence_length}_{cache_key}",
        revision.unwrap_or("main")
    );

    {
        match ENGINE_CACHE.read() {
            Ok(cache) => {
                if let Some(cached) = cache.get(&engine_key) {
                    return Ok(Arc::clone(cached));
                }
            }
            Err(poison_error) => {
                let cache = poison_error.get_ref();
                if let Some(cached) = cache.get(&engine_key) {
                    return Ok(Arc::clone(cached));
                }
            }
        }
    }

    {
        let mut cache = match ENGINE_CACHE.write() {
            Ok(guard) => guard,
            Err(poison_error) => poison_error.into_inner(),
        };

        if let Some(cached) = cache.get(&engine_key) {
            return Ok(Arc::clone(cached));
        }

        crate::ort_discovery::ensure_ort_available();

        let files = crate::onnx::download_model_files(
            repo_name,
            model_file,
            additional_files,
            revision,
            cache_dir.as_deref(),
            progress,
            Some(EMBEDDING_SHA256_MANIFEST),
            embed_err,
        )?;
        let tokenizer = crate::onnx::load_tokenizer(&files, max_sequence_length, embed_err)?;
        let session = crate::onnx::build_session(&files.model, accel.as_ref(), embed_err)?;

        let new_engine = Arc::new(EmbeddingEngine::new(tokenizer, session, pooling));
        cache.insert(engine_key, Arc::clone(&new_engine));

        Ok(new_engine)
    }
}

/// Eagerly download and cache an embedding model without returning the handle.
///
/// This triggers the same download and initialization as `get_or_init_engine`
/// but discards the result, making it suitable for cache-warming scenarios
/// where the caller doesn't need to use the model immediately. Used internally
/// by the api/mcp `cache.warm` endpoints and by the xberg-cli warm command.
/// Excluded from the language bindings via alef.toml `[exclude].functions`.
#[cfg(feature = "embeddings")]
#[cfg_attr(alef, alef(skip))]
pub fn warm_model(
    model_type: &crate::core::config::EmbeddingModelType,
    cache_dir: Option<std::path::PathBuf>,
) -> crate::Result<()> {
    let (repo, model_file, additional_files, pooling) = resolve_model_info(model_type)?;
    get_or_init_engine(
        &repo,
        &model_file,
        &additional_files,
        pooling,
        DEFAULT_EMBEDDING_MAX_SEQUENCE_LENGTH,
        cache_dir,
        // Cache warming takes an `EmbeddingModelType`, not a full config, so there is no
        // `show_download_progress` to honour here. ~keep
        crate::core::config::DownloadProgress::SILENT,
        None,
    )
    .map(|_| ())
}

/// Normalize an embedding vector in-place (L2 normalization).
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
fn normalize_in_place(embedding: &mut [f32]) {
    let magnitude: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if magnitude > f32::EPSILON {
        let inv_mag = 1.0 / magnitude;
        embedding.iter_mut().for_each(|x| *x *= inv_mag);
    }
}

/// Validate that a backend-produced batch of embeddings matches the expected
/// shape (batch size and per-vector dimension).
///
/// The dispatcher calls this on every `Plugin`-variant response before returning
/// to downstream consumers. A non-conforming backend surfaces as a
/// [`crate::XbergError::Validation`] here rather than a panic in semantic
/// chunking, `chunk.embedding` assignment, or user code.
///
/// # Errors
///
/// - [`crate::XbergError::Validation`] if `embeddings.len() != expected_count`.
/// - [`crate::XbergError::Validation`] if any `embeddings[i].len() != expected_dim`.
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
fn validate_embedding_shape(
    embeddings: &[Vec<f32>],
    expected_count: usize,
    expected_dim: usize,
    backend_name: &str,
) -> crate::Result<()> {
    if embeddings.len() != expected_count {
        return Err(crate::XbergError::Validation {
            message: format!(
                "Embedding backend '{backend_name}' returned {got} vectors for {expected} inputs",
                got = embeddings.len(),
                expected = expected_count,
            ),
            source: None,
        });
    }

    for (i, vec) in embeddings.iter().enumerate() {
        if vec.len() != expected_dim {
            return Err(crate::XbergError::Validation {
                message: format!(
                    "Embedding backend '{backend_name}' returned vector at index {i} with length {got}, expected {expected_dim}",
                    got = vec.len(),
                ),
                source: None,
            });
        }
    }

    Ok(())
}

/// Apply normalization to a batch of embeddings (parallel for large batches).
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
fn normalize_embeddings(embeddings: &mut [Vec<f32>]) {
    #[cfg(not(target_arch = "wasm32"))]
    const PARALLEL_THRESHOLD: usize = 64;
    #[cfg(not(target_arch = "wasm32"))]
    if embeddings.len() >= PARALLEL_THRESHOLD {
        use rayon::prelude::*;
        embeddings.par_iter_mut().for_each(|v| normalize_in_place(v));
        return;
    }
    embeddings.iter_mut().for_each(|v| normalize_in_place(v));
}

/// Generate embeddings for text chunks using the specified configuration.
///
/// This function modifies chunks in-place, populating their `embedding` field
/// with generated embedding vectors. It uses batch processing for efficiency.
///
/// # Arguments
///
/// * `chunks` - Mutable reference to vector of chunks to generate embeddings for
/// * `config` - Embedding configuration specifying model and parameters
///
/// # Returns
///
/// Returns `Ok(())` if embeddings were generated successfully, or an error if
/// model initialization or embedding generation fails.
#[cfg(feature = "embeddings")]
pub(crate) fn generate_embeddings_for_chunks(
    chunks: &mut [crate::types::Chunk],
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }

    let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let embeddings_result = embed_texts(&texts, config)?;

    if embeddings_result.len() != chunks.len() {
        return Err(crate::XbergError::Validation {
            message: format!(
                "Embedding generation returned {got} vectors for {expected} chunks; refusing to attach \
                 embeddings because a positional zip would misalign vectors with the wrong chunks",
                got = embeddings_result.len(),
                expected = chunks.len(),
            ),
            source: None,
        });
    }

    for (chunk, embedding) in chunks.iter_mut().zip(embeddings_result) {
        chunk.embedding = Some(embedding);
    }

    Ok(())
}

/// Generate embeddings for a list of raw text strings (standalone, no chunking pipeline).
///
/// Returns one embedding vector per input text, in the same order as the input.
/// Uses the same model resolution, engine caching, and batch processing as the
/// chunking pipeline. Normalization is applied if `config.normalize` is true.
///
/// # Arguments
///
/// * `texts` - Slice of strings to embed
/// * `config` - Embedding configuration specifying model, batch size, and normalization
///
/// # Returns
///
/// Returns `Vec<Vec<f32>>` — one `Vec<f32>` per input text. Returns an empty
/// `Vec` if `texts` is empty (no error).
///
/// # Errors
///
/// - `XbergError::MissingDependency` if ONNX Runtime is not installed
/// - `XbergError::Embedding` if the preset name is unknown or model download fails
///
/// # Example
///
/// ```rust,ignore
/// use xberg::{embed_texts, EmbeddingConfig, EmbeddingModelType};
///
/// let config = EmbeddingConfig {
///     model: EmbeddingModelType::Preset { name: "balanced".to_string() },
///     normalize: true,
///     ..Default::default()
/// };
/// let embeddings = embed_texts(&["Hello, world!", "Second text"], &config)?;
/// assert_eq!(embeddings.len(), 2);
/// assert_eq!(embeddings[0].len(), 768); // balanced preset = 768 dims
/// ```
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
#[doc(hidden)]
pub fn embed_texts<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    for (i, t) in texts.iter().enumerate() {
        if t.as_ref().is_empty() {
            return Err(crate::XbergError::embedding(format!(
                "Text at position {pos} is empty. All texts must be non-empty.",
                pos = i + 1
            )));
        }
    }

    match &config.model {
        #[cfg(all(feature = "liter-llm", feature = "tokio-runtime", not(target_arch = "wasm32")))]
        crate::core::config::EmbeddingModelType::Llm { llm } => {
            let normalize = config.normalize;
            let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
                tokio::task::block_in_place(|| {
                    handle.block_on(crate::llm::vlm_embeddings::embed_via_llm(texts, llm, normalize))
                })
            } else {
                crate::core::runtime::global_runtime()?
                    .block_on(crate::llm::vlm_embeddings::embed_via_llm(texts, llm, normalize))
            };
            result.map(|(embeddings, _usage)| embeddings)
        }
        #[cfg(target_arch = "wasm32")]
        crate::core::config::EmbeddingModelType::Llm { .. } => Err(crate::XbergError::MissingDependency(
            "LLM embeddings are not available on wasm builds".into(),
        )),
        #[cfg(all(
            not(target_arch = "wasm32"),
            any(not(feature = "liter-llm"), not(feature = "tokio-runtime"))
        ))]
        crate::core::config::EmbeddingModelType::Llm { .. } => Err(crate::XbergError::MissingDependency(
            "LLM embeddings require the 'liter-llm' and 'tokio-runtime' features. Rebuild with --features liter-llm"
                .into(),
        )),
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        crate::core::config::EmbeddingModelType::Plugin { name } => {
            let registry = crate::plugins::get_embedding_backend_registry();
            let (backend, expected_dim) = {
                let guard = registry.read();
                guard.get_with_dimensions(name)?
            };
            let expected_count = texts.len();
            let owned_texts: Vec<String> = texts.iter().map(|t| t.as_ref().to_string()).collect();

            let timeout = config
                .max_embed_duration_secs
                .filter(|&s| s > 0)
                .map(std::time::Duration::from_secs);
            let embed_future = async {
                match timeout {
                    Some(dur) => tokio::time::timeout(dur, backend.embed(owned_texts))
                        .await
                        .map_err(|_| crate::XbergError::Plugin {
                            message: format!("Embedding backend '{name}' did not complete within {dur:?}"),
                            plugin_name: name.clone(),
                        })?,
                    None => backend.embed(owned_texts).await,
                }
            };
            let embed_result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
                tokio::task::block_in_place(|| handle.block_on(embed_future))
            } else {
                crate::core::runtime::global_runtime()?.block_on(embed_future)
            };
            let mut embeddings = embed_result?;

            validate_embedding_shape(&embeddings, expected_count, expected_dim, name)?;

            if config.normalize {
                normalize_embeddings(&mut embeddings);
            }

            Ok(embeddings)
        }
        #[cfg(target_arch = "wasm32")]
        crate::core::config::EmbeddingModelType::Plugin { .. } => Err(crate::XbergError::MissingDependency(
            "Synchronous plugin embeddings are not available on wasm builds; use embed_texts_async instead".into(),
        )),
        #[cfg(all(not(feature = "tokio-runtime"), not(target_arch = "wasm32")))]
        crate::core::config::EmbeddingModelType::Plugin { .. } => Err(crate::XbergError::MissingDependency(
            "Plugin embedding backends require the 'tokio-runtime' feature. Rebuild with --features tokio-runtime"
                .into(),
        )),
        crate::core::config::EmbeddingModelType::Preset { .. }
        | crate::core::config::EmbeddingModelType::Custom { .. } => embed_texts_local(texts, config),
    }
}

/// Local (non-hosted) dispatch for `Preset`/`Custom` model types: resolves the
/// preset's [`EmbeddingBackend`] and routes to either the ONNX engine or the
/// pure-Rust static (model2vec) engine.
///
/// Split out of [`embed_texts`] so each backend's `#[cfg]` block stays a single
/// self-contained arm instead of interleaving `cfg` attributes mid-match.
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
fn embed_texts_local<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    let backend = resolve_local_backend(&config.model)?;

    match backend {
        #[cfg(feature = "embeddings")]
        EmbeddingBackend::Onnx => embed_texts_onnx(texts, config),
        #[cfg(not(feature = "embeddings"))]
        EmbeddingBackend::Onnx => Err(crate::XbergError::MissingDependency(
            "ONNX-backed embedding presets require the 'embeddings' feature. Rebuild with --features embeddings".into(),
        )),
        #[cfg(feature = "static-embeddings")]
        EmbeddingBackend::Static => embed_texts_static(texts, config),
        #[cfg(not(feature = "static-embeddings"))]
        EmbeddingBackend::Static => Err(crate::XbergError::MissingDependency(
            "Static (model2vec) embedding presets require the 'static-embeddings' feature. \
             Rebuild with --features static-embeddings"
                .into(),
        )),
    }
}

/// Resolve which [`EmbeddingBackend`] a `Preset`/`Custom` model type runs on.
///
/// `Custom` model types have no preset metadata to consult — they always
/// target the ONNX path (matches the historical behavior of `resolve_model_info`
/// / `get_or_init_engine`, which assume an ONNX-shaped custom HF repo).
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
fn resolve_local_backend(model_type: &crate::core::config::EmbeddingModelType) -> crate::Result<EmbeddingBackend> {
    match model_type {
        crate::core::config::EmbeddingModelType::Preset { name } => get_preset(name)
            .map(|p| p.backend)
            .ok_or_else(|| crate::XbergError::embedding(format!("Unknown embedding preset: {name}"))),
        crate::core::config::EmbeddingModelType::Custom { .. } => Ok(EmbeddingBackend::Onnx),
        crate::core::config::EmbeddingModelType::Llm { .. }
        | crate::core::config::EmbeddingModelType::Plugin { .. } => {
            unreachable!("Llm and Plugin model types are dispatched before embed_texts_local is called")
        }
    }
}

/// ONNX-backed local embedding path (the historical `Preset`/`Custom` behavior).
#[cfg(feature = "embeddings")]
fn embed_texts_onnx<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    let chunk_count = texts.len();
    let (repo, model_file, additional_files, pooling) = resolve_model_info(&config.model)?;
    let engine = get_or_init_engine(
        &repo,
        &model_file,
        &additional_files,
        pooling,
        config
            .max_sequence_length
            .unwrap_or(DEFAULT_EMBEDDING_MAX_SEQUENCE_LENGTH),
        config.cache_dir.clone(),
        config.into(),
        config.acceleration.clone(),
    )?;

    let text_refs: Vec<&str> = texts.iter().map(|t| t.as_ref()).collect();
    let mut embeddings = engine.embed(&text_refs, config.batch_size).map_err(|e| {
        crate::XbergError::embedding(format!(
            "Failed to generate embeddings for {chunk_count} texts (model={:?}, batch_size={}): {e}",
            config.model, config.batch_size
        ))
    })?;

    if config.normalize {
        normalize_embeddings(&mut embeddings);
    }

    Ok(embeddings)
}

/// Pure-Rust static (model2vec) local embedding path — no ONNX Runtime. The
/// only dense-embedding backend available on `no-ort-target` (WASM, Android
/// x86_64 emulator).
#[cfg(feature = "static-embeddings")]
fn embed_texts_static<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    let crate::core::config::EmbeddingModelType::Preset { name } = &config.model else {
        return Err(crate::XbergError::embedding(
            "Static embedding backend only supports EmbeddingModelType::Preset, not Custom".to_string(),
        ));
    };
    let preset =
        get_preset(name).ok_or_else(|| crate::XbergError::embedding(format!("Unknown embedding preset: {name}")))?;

    let cache_directory = static_engine_cache_dir(config.cache_dir.clone());
    let engine = get_or_init_static_engine(
        &preset.model_repo,
        &preset.model_file,
        cache_directory.as_deref(),
        config.into(),
    )?;

    let text_refs: Vec<&str> = texts.iter().map(|t| t.as_ref()).collect();
    let mut embeddings = engine.embed(&text_refs, config.batch_size, config.max_sequence_length);

    validate_embedding_shape(&embeddings, texts.len(), preset.dimensions, &preset.name)?;

    if config.normalize {
        normalize_embeddings(&mut embeddings);
    }

    Ok(embeddings)
}

/// Preserve an explicit alternate Hugging Face cache root. `None` delegates
/// standard cache discovery to hf-hub.
#[cfg(feature = "static-embeddings")]
fn static_engine_cache_dir(cache_dir: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    cache_dir
}

#[cfg(all(feature = "static-embeddings", not(target_arch = "wasm32")))]
fn static_engine_cache_key(cache_dir: Option<&std::path::Path>) -> String {
    crate::model_download::hf_cache_key(cache_dir)
}

#[cfg(all(feature = "static-embeddings", target_arch = "wasm32"))]
fn static_engine_cache_key(cache_dir: Option<&std::path::Path>) -> String {
    cache_dir
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "wasm-no-hf-cache".to_string())
}

#[cfg(feature = "static-embeddings")]
type CachedStaticEngine = std::sync::Arc<static_engine::StaticEmbeddingEngine>;

#[cfg(feature = "static-embeddings")]
static STATIC_ENGINE_CACHE: LazyLock<std::sync::RwLock<ahash::AHashMap<String, CachedStaticEngine>>> =
    LazyLock::new(|| std::sync::RwLock::new(ahash::AHashMap::new()));

/// Get or initialize a static-embedding engine from cache, downloading model
/// files on first use (native/Android only — see [`static_engine`]).
#[cfg(feature = "static-embeddings")]
fn get_or_init_static_engine(
    repo_name: &str,
    model_file: &str,
    cache_directory: Option<&std::path::Path>,
    progress: crate::core::config::DownloadProgress,
) -> crate::Result<CachedStaticEngine> {
    let cache_key = static_engine_cache_key(cache_directory);
    let engine_key = format!("{repo_name}_{model_file}_{EMBEDDING_MODEL_REVISION}_{cache_key}");

    {
        match STATIC_ENGINE_CACHE.read() {
            Ok(cache) => {
                if let Some(cached) = cache.get(&engine_key) {
                    return Ok(std::sync::Arc::clone(cached));
                }
            }
            Err(poison) => {
                if let Some(cached) = poison.get_ref().get(&engine_key) {
                    return Ok(std::sync::Arc::clone(cached));
                }
            }
        }
    }

    let mut cache = match STATIC_ENGINE_CACHE.write() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    };
    if let Some(cached) = cache.get(&engine_key) {
        return Ok(std::sync::Arc::clone(cached));
    }

    let engine = std::sync::Arc::new(static_engine::download_and_build(
        repo_name,
        model_file,
        cache_directory,
        progress,
    )?);
    cache.insert(engine_key, std::sync::Arc::clone(&engine));
    Ok(engine)
}

/// Generate embeddings asynchronously for a list of text strings.
///
/// This is the async counterpart to [`embed_texts`]. It offloads the blocking
/// ONNX inference work to a dedicated blocking thread pool via Tokio's
/// `spawn_blocking`, keeping the async executor free.
///
/// Returns one embedding vector per input text in the same order.
///
/// # Arguments
///
/// * `texts` - Vec of strings to embed (owned, sent to blocking thread)
/// * `config` - Embedding configuration specifying model, batch size, and normalization
///
/// # Errors
///
/// - `XbergError::MissingDependency` if ONNX Runtime is not installed
/// - `XbergError::Embedding` if the preset name is unknown, model download fails,
///   or the blocking inference task panics
///
/// # Example
///
/// ```rust,ignore
/// use xberg::{embed_texts_async, EmbeddingConfig};
///
/// let embeddings = embed_texts_async(
///     vec!["Hello!".to_string()],
///     &EmbeddingConfig::default(),
/// ).await?;
/// ```
#[cfg(all(
    feature = "tokio-runtime",
    any(feature = "embeddings", feature = "static-embeddings")
))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_texts_async<T: AsRef<str> + Send + 'static>(
    texts: Vec<T>,
    config: &crate::core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    for (i, t) in texts.iter().enumerate() {
        if t.as_ref().is_empty() {
            return Err(crate::XbergError::embedding(format!(
                "Text at position {pos} is empty. All texts must be non-empty.",
                pos = i + 1
            )));
        }
    }

    match &config.model {
        #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
        crate::core::config::EmbeddingModelType::Llm { llm } => {
            return crate::llm::vlm_embeddings::embed_via_llm(&texts, llm, config.normalize)
                .await
                .map(|(embeddings, _usage)| embeddings);
        }
        #[cfg(target_arch = "wasm32")]
        crate::core::config::EmbeddingModelType::Llm { .. } => {
            return Err(crate::XbergError::MissingDependency(
                "LLM embeddings are not available on wasm builds".into(),
            ));
        }
        #[cfg(all(not(feature = "liter-llm"), not(target_arch = "wasm32")))]
        crate::core::config::EmbeddingModelType::Llm { .. } => {
            return Err(crate::XbergError::MissingDependency(
                "LLM embeddings require the 'liter-llm' feature. Rebuild with --features liter-llm".into(),
            ));
        }
        crate::core::config::EmbeddingModelType::Plugin { name } => {
            let registry = crate::plugins::get_embedding_backend_registry();
            let (backend, expected_dim) = {
                let guard = registry.read();
                guard.get_with_dimensions(name)?
            };
            let expected_count = texts.len();
            let owned_texts: Vec<String> = texts.iter().map(|t| t.as_ref().to_string()).collect();
            let timeout = config
                .max_embed_duration_secs
                .filter(|&s| s > 0)
                .map(std::time::Duration::from_secs);
            let mut embeddings = match timeout {
                Some(dur) => tokio::time::timeout(dur, backend.embed(owned_texts))
                    .await
                    .map_err(|_| crate::XbergError::Plugin {
                        message: format!("Embedding backend '{name}' did not complete within {dur:?}"),
                        plugin_name: name.clone(),
                    })??,
                None => backend.embed(owned_texts).await?,
            };
            validate_embedding_shape(&embeddings, expected_count, expected_dim, name)?;
            if config.normalize {
                normalize_embeddings(&mut embeddings);
            }
            return Ok(embeddings);
        }
        crate::core::config::EmbeddingModelType::Preset { .. }
        | crate::core::config::EmbeddingModelType::Custom { .. } => {}
    }

    let _permit = EMBED_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| crate::XbergError::embedding("Embedding semaphore closed".to_string()))?;

    #[cfg(not(target_arch = "wasm32"))]
    {
        let config = Arc::new(config.clone());
        tokio::task::spawn_blocking(move || embed_texts(&texts, &config))
            .await
            .map_err(|e| crate::XbergError::embedding(format!("Embedding task panicked: {e}")))?
    }
    #[cfg(target_arch = "wasm32")]
    {
        embed_texts(&texts, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fail-closed guarantee: every hosted preset's weight file (and any external-data
    /// sibling) must be pinned in `presets.sha256sum`, so `download_model_files` can
    /// verify it. Guards against a preset being added without a matching manifest entry.
    #[test]
    fn every_preset_file_is_pinned_in_manifest() {
        let manifest = crate::model_download::parse_sha256_manifest(EMBEDDING_SHA256_MANIFEST).unwrap();
        let pinned: std::collections::HashSet<&str> = manifest.iter().map(|(p, _)| p.as_str()).collect();
        for preset in EMBEDDING_PRESETS.iter() {
            assert!(
                pinned.contains(preset.model_file.as_str()),
                "preset {} model_file {} is not pinned in presets.sha256sum",
                preset.name,
                preset.model_file
            );
            for sibling in &preset.additional_files {
                assert!(
                    pinned.contains(sibling.as_str()),
                    "preset {} additional file {} is not pinned in presets.sha256sum",
                    preset.name,
                    sibling
                );
            }

            let model_dir = std::path::Path::new(&preset.model_file)
                .parent()
                .and_then(|p| p.to_str())
                .filter(|s| !s.is_empty());
            let companion_path = |name: &str| match model_dir {
                Some(dir) => format!("{dir}/{name}"),
                None => name.to_string(),
            };
            for required in ["tokenizer.json", "config.json"] {
                let path = companion_path(required);
                assert!(
                    pinned.contains(path.as_str()),
                    "preset {} companion {} is not pinned in presets.sha256sum",
                    preset.name,
                    path
                );
            }
        }
    }

    #[test]
    fn test_get_preset() {
        assert!(get_preset("balanced").is_some());
        assert!(get_preset("fast").is_some());
        assert!(get_preset("quality").is_some());
        assert!(get_preset("multilingual").is_some());
        assert!(get_preset("gte-modernbert-base").is_some());
        assert!(get_preset("lightweight").is_some());
        assert!(get_preset("nonexistent").is_none());
    }

    #[test]
    fn test_list_presets() {
        let presets = list_presets();
        assert_eq!(presets.len(), 8, "expected exactly 8 presets, got: {presets:?}");
        assert!(presets.iter().any(|n| n == "fast"));
        assert!(presets.iter().any(|n| n == "balanced"));
        assert!(presets.iter().any(|n| n == "quality"));
        assert!(presets.iter().any(|n| n == "multilingual"));
        assert!(presets.iter().any(|n| n == "gte-modernbert-base"));
        assert!(presets.iter().any(|n| n == "lightweight"));
        assert!(presets.iter().any(|n| n == "arctic-embed-m-v2.0"));
        assert!(presets.iter().any(|n| n == "qwen3-embedding-0.6b"));
    }

    #[test]
    fn asymmetric_presets_carry_query_prefix_and_external_data() {
        let arctic = get_preset("arctic-embed-m-v2.0").expect("arctic preset must exist");
        assert_eq!(arctic.query_prefix.as_deref(), Some("query: "));
        assert_eq!(arctic.pooling, "cls");
        assert_eq!(arctic.dimensions, 768);
        assert_eq!(
            arctic.additional_files,
            vec!["arctic-embed-m-v2.0/model.onnx.data".to_string()]
        );

        let qwen3 = get_preset("qwen3-embedding-0.6b").expect("qwen3-embedding preset must exist");
        assert_eq!(qwen3.query_prefix, None);
        assert_eq!(qwen3.pooling, "last");
        assert_eq!(qwen3.dimensions, 1024);
        assert_eq!(
            qwen3.additional_files,
            vec!["qwen3-embedding-0.6b/model.onnx.data".to_string()]
        );
    }

    #[test]
    fn lightweight_preset_uses_static_backend() {
        let preset = get_preset("lightweight").expect("lightweight preset must exist");
        assert_eq!(preset.backend, EmbeddingBackend::Static);
        assert_eq!(preset.dimensions, 256);
        assert_eq!(preset.model_repo, "xberg-io/embedding-models");
    }

    #[test]
    fn every_onnx_preset_defaults_to_onnx_backend() {
        for preset in EMBEDDING_PRESETS.iter().filter(|p| p.name != "lightweight") {
            assert_eq!(
                preset.backend,
                EmbeddingBackend::Onnx,
                "preset '{}' should default to the Onnx backend",
                preset.name
            );
        }
    }

    #[test]
    fn embedding_backend_deserializes_missing_field_as_onnx() {
        // deserialize, defaulting to Onnx via #[serde(default)].
        let json = r#"{
            "name": "custom",
            "chunk_size": 512,
            "overlap": 50,
            "model_repo": "org/repo",
            "pooling": "mean",
            "model_file": "model.onnx",
            "dimensions": 384,
            "description": "test"
        }"#;
        let preset: EmbeddingPreset = serde_json::from_str(json).expect("should deserialize without backend field");
        assert_eq!(preset.backend, EmbeddingBackend::Onnx);
    }

    #[test]
    fn test_preset_dimensions() {
        let balanced = get_preset("balanced").unwrap();
        assert_eq!(balanced.dimensions, 768);

        let fast = get_preset("fast").unwrap();
        assert_eq!(fast.dimensions, 384);

        let quality = get_preset("quality").unwrap();
        assert_eq!(quality.dimensions, 1024);
    }

    #[test]
    fn test_preset_chunk_sizes() {
        let fast = get_preset("fast").unwrap();
        assert_eq!(fast.chunk_size, 512);
        assert_eq!(fast.overlap, 50);

        let quality = get_preset("quality").unwrap();
        assert_eq!(quality.chunk_size, 2000);
        assert_eq!(quality.overlap, 200);
    }

    #[test]
    fn test_preset_model_repos() {
        let fast = get_preset("fast").unwrap();
        assert_eq!(fast.model_repo, "xberg-io/embedding-models");
        assert_eq!(fast.pooling, "mean");
        assert_eq!(fast.model_file, "all-MiniLM-L6-v2/model_quantized.onnx");

        let balanced = get_preset("balanced").unwrap();
        assert_eq!(balanced.model_repo, "xberg-io/embedding-models");
        assert_eq!(balanced.pooling, "cls");
    }

    #[test]
    fn test_embed_texts_rejects_empty_string() {
        let config = crate::core::config::EmbeddingConfig::default();
        let texts = vec!["valid", ""];
        let err = embed_texts(&texts, &config).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("position 2"),
            "Error should identify the empty text position, got: {msg}"
        );
        assert!(msg.contains("empty"), "Error should mention empty text, got: {msg}");
    }

    #[test]
    fn test_embed_texts_empty_list_returns_empty() {
        let config = crate::core::config::EmbeddingConfig::default();
        let texts: Vec<&str> = vec![];
        let result = embed_texts(&texts, &config).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_embed_texts_rejects_first_empty_string() {
        let config = crate::core::config::EmbeddingConfig::default();
        let texts = vec![""];
        let err = embed_texts(&texts, &config).unwrap_err();
        assert!(err.to_string().contains("position 1"));
    }

    /// Regression test for #713: embed_texts called from inside a tokio runtime
    /// (e.g. server mode) must not panic with "cannot block inside runtime".
    /// The LLM path will fail with MissingDependency or a connection error,
    /// but it must NOT panic.
    #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn test_embed_texts_llm_inside_runtime_does_not_panic() {
        let config = crate::core::config::EmbeddingConfig {
            model: crate::core::config::EmbeddingModelType::Llm {
                llm: Box::new(crate::core::config::LlmConfig {
                    model: "openai/text-embedding-3-small".to_string(),
                    api_key: Some("invalid-key-for-test".to_string()),
                    ..Default::default()
                }),
            },
            ..Default::default()
        };
        let result = tokio::task::spawn_blocking(move || embed_texts(&["test text"], &config)).await;
        assert!(result.is_ok(), "spawn_blocking should not panic");
        assert!(result.unwrap().is_err(), "Expected auth error, not success");
    }

    /// Regression test for #683: GraphOptimizationLevel::Level3 maps to
    /// ORT_ENABLE_LAYOUT (3), only valid in ORT >= 1.21. The correct variant
    /// for "all optimisations" is ::All (ORT_ENABLE_ALL = 99), valid across
    /// every ORT 1.x release.
    #[cfg(feature = "embeddings")]
    #[test]
    fn test_ort_optimization_level_all_not_level3() {
        use ort::session::builder::GraphOptimizationLevel;
        let all_repr = format!("{:?}", GraphOptimizationLevel::All);
        let level3_repr = format!("{:?}", GraphOptimizationLevel::Level3);
        assert_eq!(all_repr, "All");
        assert_ne!(level3_repr, "All", "Level3 must not be the same variant as All");
    }

    // uses `#[tokio::test]` throughout. A `static-embeddings`-only build (no
    #[cfg(feature = "tokio-runtime")]
    mod plugin_dispatch {
        use crate::plugins::embedding::{register_embedding_backend, unregister_embedding_backend};
        use crate::plugins::{EmbeddingBackend, Plugin};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        fn unique_name(suffix: &str) -> String {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            format!("dispatch-{suffix}-{id}")
        }

        /// Backend whose `embed` response shape is fully parameterised so tests
        /// can exercise the validation paths (length mismatch, dim mismatch).
        struct ConfigurableBackend {
            name: String,
            reported_dimensions: usize,
            vector_dimensions: usize,
            response_count: Option<usize>,
            panic_on_embed: bool,
            fill_value: f32,
        }

        impl Plugin for ConfigurableBackend {
            fn name(&self) -> &str {
                &self.name
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl EmbeddingBackend for ConfigurableBackend {
            fn dimensions(&self) -> usize {
                self.reported_dimensions
            }

            async fn embed(&self, texts: Vec<String>) -> crate::Result<Vec<Vec<f32>>> {
                if self.panic_on_embed {
                    return Err(crate::XbergError::Plugin {
                        message: "simulated backend failure".to_string(),
                        plugin_name: self.name.clone(),
                    });
                }
                let count = self.response_count.unwrap_or(texts.len());
                Ok((0..count)
                    .map(|_| vec![self.fill_value; self.vector_dimensions])
                    .collect())
            }
        }

        fn config_for(name: &str, normalize: bool) -> crate::core::config::EmbeddingConfig {
            crate::core::config::EmbeddingConfig {
                model: crate::core::config::EmbeddingModelType::Plugin { name: name.to_string() },
                normalize,
                ..Default::default()
            }
        }

        #[test]
        fn dispatches_to_registered_backend() {
            let name = unique_name("happy");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 4,
                vector_dimensions: 4,
                response_count: None,
                panic_on_embed: false,
                fill_value: 0.25,
            }))
            .unwrap();

            let vectors = super::super::embed_texts(&["a", "b", "c"], &config_for(&name, false)).unwrap();
            assert_eq!(vectors.len(), 3);
            assert!(vectors.iter().all(|v| v.len() == 4 && v[0] == 0.25));

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn unknown_plugin_name_errors() {
            let config = config_for("never-registered-x", false);
            let err = super::super::embed_texts(&["a"], &config).unwrap_err();
            assert!(matches!(err, crate::XbergError::Plugin { .. }));
        }

        /// Regression: the synchronous `embed_texts` must work when invoked from
        /// inside a multi-thread Tokio runtime (e.g. a server's `spawn_blocking`
        /// task). The previous implementation built a per-call current-thread
        /// runtime and dropped it inside the caller's blocking context, panicking
        /// with "Cannot drop a runtime in a context where blocking is not allowed".
        /// Routing through the shared, never-dropped global runtime removes that.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn embed_texts_inside_multi_thread_runtime_does_not_panic() {
            let name = unique_name("rt-safe");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 4,
                vector_dimensions: 4,
                response_count: None,
                panic_on_embed: false,
                fill_value: 0.5,
            }))
            .unwrap();

            let cfg = config_for(&name, false);
            let vectors = tokio::task::spawn_blocking(move || super::super::embed_texts(&["a", "b"], &cfg))
                .await
                .expect("spawn_blocking task must not panic")
                .expect("embedding must succeed");
            assert_eq!(vectors.len(), 2);
            assert!(vectors.iter().all(|v| v.len() == 4 && v[0] == 0.5));

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn length_mismatch_surfaces_as_validation_error() {
            let name = unique_name("len-mismatch");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 3,
                vector_dimensions: 3,
                response_count: Some(2),
                panic_on_embed: false,
                fill_value: 0.0,
            }))
            .unwrap();

            let err = super::super::embed_texts(&["a", "b", "c"], &config_for(&name, false)).unwrap_err();
            let msg = err.to_string();
            assert!(
                matches!(err, crate::XbergError::Validation { .. }),
                "expected Validation error, got {err:?}"
            );
            assert!(msg.contains('2') && msg.contains('3'), "message: {msg}");

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn dimension_mismatch_surfaces_as_validation_error() {
            let name = unique_name("dim-mismatch");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 4,
                vector_dimensions: 5,
                response_count: None,
                panic_on_embed: false,
                fill_value: 0.0,
            }))
            .unwrap();

            let err = super::super::embed_texts(&["a", "b"], &config_for(&name, false)).unwrap_err();
            assert!(matches!(err, crate::XbergError::Validation { .. }));
            let msg = err.to_string();
            assert!(msg.contains("index 0"), "message should cite bad index: {msg}");

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn backend_error_surfaces_as_plugin_error() {
            let name = unique_name("err");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 3,
                vector_dimensions: 3,
                response_count: None,
                panic_on_embed: true,
                fill_value: 0.0,
            }))
            .unwrap();

            let err = super::super::embed_texts(&["a"], &config_for(&name, false)).unwrap_err();
            assert!(matches!(err, crate::XbergError::Plugin { .. }));
            assert!(err.to_string().contains("simulated backend failure"));

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn empty_texts_short_circuits_before_backend_call() {
            let config = config_for("never-looked-up", false);
            let texts: Vec<&str> = vec![];
            let vectors = super::super::embed_texts(&texts, &config).unwrap();
            assert!(vectors.is_empty());
        }

        #[test]
        fn concurrent_registration_stress() {
            use std::thread;
            let mut handles = Vec::new();
            let prefix = unique_name("stress");
            for t in 0..8 {
                let prefix = prefix.clone();
                handles.push(thread::spawn(move || {
                    for i in 0..10 {
                        let name = format!("{prefix}-t{t}-i{i}");
                        register_embedding_backend(Arc::new(ConfigurableBackend {
                            name: name.clone(),
                            reported_dimensions: 2,
                            vector_dimensions: 2,
                            response_count: None,
                            panic_on_embed: false,
                            fill_value: 0.5,
                        }))
                        .unwrap();
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }

            let list = crate::plugins::embedding::list_embedding_backends().unwrap();
            let registered = list.iter().filter(|n| n.starts_with(&prefix)).count();
            assert_eq!(registered, 80, "expected 80 registrations, got {registered}");

            let sample = format!("{prefix}-t0-i0");
            let vectors = super::super::embed_texts(&["probe"], &config_for(&sample, false)).unwrap();
            assert_eq!(vectors.len(), 1);

            for t in 0..8 {
                for i in 0..10 {
                    let name = format!("{prefix}-t{t}-i{i}");
                    let _ = crate::plugins::embedding::unregister_embedding_backend(&name);
                }
            }
        }

        /// Backend that sleeps longer than the configured timeout — exercises
        /// the tokio::time::timeout wrapper in the dispatch arm.
        struct SlowBackend {
            name: String,
            sleep_duration: std::time::Duration,
        }

        impl Plugin for SlowBackend {
            fn name(&self) -> &str {
                &self.name
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl EmbeddingBackend for SlowBackend {
            fn dimensions(&self) -> usize {
                4
            }

            async fn embed(&self, texts: Vec<String>) -> crate::Result<Vec<Vec<f32>>> {
                tokio::time::sleep(self.sleep_duration).await;
                Ok(texts.iter().map(|_| vec![0.0; 4]).collect())
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn timeout_fires_when_backend_exceeds_duration() {
            let name = unique_name("timeout");
            register_embedding_backend(Arc::new(SlowBackend {
                name: name.clone(),
                sleep_duration: std::time::Duration::from_secs(2),
            }))
            .unwrap();

            let config = crate::core::config::EmbeddingConfig {
                model: crate::core::config::EmbeddingModelType::Plugin { name: name.clone() },
                max_embed_duration_secs: Some(1),
                ..Default::default()
            };

            let err = super::super::embed_texts(&["probe"], &config).expect_err("timeout should fire");
            assert!(
                matches!(err, crate::XbergError::Plugin { .. }),
                "expected Plugin error, got {err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("did not complete within"),
                "error message should mention timeout; got: {msg}"
            );

            unregister_embedding_backend(&name).unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn async_dispatch_applies_normalization_when_enabled() {
            let name = unique_name("async-normalize");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 2,
                vector_dimensions: 2,
                response_count: None,
                panic_on_embed: false,
                fill_value: 3.0,
            }))
            .unwrap();

            let texts: Vec<String> = vec!["probe".to_string()];
            let vectors = super::super::embed_texts_async(texts, &config_for(&name, true))
                .await
                .expect("async dispatch should succeed");
            let v = &vectors[0];
            let mag = (v[0] * v[0] + v[1] * v[1]).sqrt();
            assert!(
                (mag - 1.0).abs() < 1e-6,
                "expected unit-norm after normalize=true on async path; got mag={mag}"
            );

            unregister_embedding_backend(&name).unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn async_dispatch_smoke_test() {
            let name = unique_name("async-path");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 3,
                vector_dimensions: 3,
                response_count: None,
                panic_on_embed: false,
                fill_value: 0.5,
            }))
            .unwrap();

            let config = config_for(&name, false);
            let texts: Vec<String> = vec!["x".to_string(), "y".to_string()];
            let vectors = super::super::embed_texts_async(texts, &config)
                .await
                .expect("async dispatch should succeed");
            assert_eq!(vectors.len(), 2);
            assert!(vectors.iter().all(|v| v.len() == 3 && v[0] == 0.5));

            unregister_embedding_backend(&name).unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn disabled_timeout_allows_slow_backend_to_complete() {
            let name = unique_name("no-timeout");
            register_embedding_backend(Arc::new(SlowBackend {
                name: name.clone(),
                sleep_duration: std::time::Duration::from_millis(100),
            }))
            .unwrap();

            let config = crate::core::config::EmbeddingConfig {
                model: crate::core::config::EmbeddingModelType::Plugin { name: name.clone() },
                max_embed_duration_secs: None,
                ..Default::default()
            };

            let result = super::super::embed_texts(&["probe"], &config);
            assert!(result.is_ok(), "expected Ok with timeout disabled; got {result:?}");

            unregister_embedding_backend(&name).unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn zero_max_duration_treated_as_disabled() {
            let name = unique_name("zero-timeout");
            register_embedding_backend(Arc::new(SlowBackend {
                name: name.clone(),
                sleep_duration: std::time::Duration::from_millis(50),
            }))
            .unwrap();

            let config = crate::core::config::EmbeddingConfig {
                model: crate::core::config::EmbeddingModelType::Plugin { name: name.clone() },
                max_embed_duration_secs: Some(0),
                ..Default::default()
            };

            let result = super::super::embed_texts(&["probe"], &config);
            assert!(
                result.is_ok(),
                "expected Ok with Some(0) treated as disabled; got {result:?}"
            );

            unregister_embedding_backend(&name).unwrap();
        }

        #[test]
        fn normalization_applied_when_enabled() {
            let name = unique_name("normalize");
            register_embedding_backend(Arc::new(ConfigurableBackend {
                name: name.clone(),
                reported_dimensions: 2,
                vector_dimensions: 2,
                response_count: None,
                panic_on_embed: false,
                fill_value: 3.0,
            }))
            .unwrap();

            let vectors = super::super::embed_texts(&["a"], &config_for(&name, true)).unwrap();
            let v = &vectors[0];
            let mag = (v[0] * v[0] + v[1] * v[1]).sqrt();
            assert!(
                (mag - 1.0).abs() < 1e-6,
                "expected unit-norm after normalize=true, got mag={mag}"
            );

            unregister_embedding_backend(&name).unwrap();
        }
    }

    #[test]
    fn validate_shape_accepts_correct_response() {
        let embeddings = vec![vec![0.0; 4]; 3];
        super::validate_embedding_shape(&embeddings, 3, 4, "ok").unwrap();
    }

    #[test]
    fn validate_shape_rejects_count_mismatch() {
        let embeddings = vec![vec![0.0; 4]; 2];
        let err = super::validate_embedding_shape(&embeddings, 3, 4, "bad-count").unwrap_err();
        assert!(matches!(err, crate::XbergError::Validation { .. }));
    }

    #[test]
    fn validate_shape_rejects_dim_mismatch() {
        let embeddings = vec![vec![0.0; 4], vec![0.0; 3], vec![0.0; 4]];
        let err = super::validate_embedding_shape(&embeddings, 3, 4, "bad-dim").unwrap_err();
        assert!(matches!(err, crate::XbergError::Validation { .. }));
        assert!(err.to_string().contains("index 1"));
    }

    #[test]
    fn validate_shape_empty_expected_count_ok() {
        super::validate_embedding_shape(&[], 0, 4, "empty").unwrap();
    }
}
