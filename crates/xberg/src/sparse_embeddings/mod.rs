//! Sparse (SPLADE) learned embeddings.
//!
//! Produces high-dimensional, mostly-zero vocabulary vectors from a
//! `BertForMaskedLM` ONNX model, stored as parallel `(indices, values)` arrays.
//! These enable hybrid dense+sparse retrieval: a sparse arm captures exact
//! lexical term importance that dense vectors smear away.
//!
//! Built on the shared [`crate::onnx`] model-loading helpers. The engine math is
//! in [`engine`].
//!
//! Since v5.0.0.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

#[cfg(feature = "sparse-embeddings")]
pub mod engine;

#[cfg(feature = "sparse-embeddings")]
use std::sync::{Arc, RwLock};

#[cfg(feature = "sparse-embeddings")]
use ahash::AHashMap;
#[cfg(feature = "sparse-embeddings")]
use engine::SparseEmbeddingEngine;

/// Default ONNX file for a `Custom` SPLADE repo when none is specified.
#[cfg(feature = "sparse-embeddings")]
const DEFAULT_MODEL_FILE: &str = "onnx/model.onnx";

/// A sparse learned embedding: vocabulary term indices and their weights.
///
/// `indices` are ascending vocabulary token ids; `values[i]` is the weight for
/// `indices[i]`. The two arrays always have equal length. Only strictly-positive
/// terms are retained, so the representation is genuinely sparse.
///
/// Since v5.0.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct SparseEmbedding {
    /// Vocabulary token ids with non-zero weight, ascending.
    pub indices: Vec<u32>,
    /// Weights parallel to [`SparseEmbedding::indices`].
    pub values: Vec<f32>,
}

/// Static metadata for a bundled SPLADE preset (WASM/Android-safe, no ORT).
///
/// Since v5.0.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct SparseEmbeddingPreset {
    /// Stable preset name referenced from config.
    pub name: String,
    /// HuggingFace repository hosting the ONNX model.
    pub model_repo: String,
    /// Path to the ONNX file within the repo.
    pub model_file: String,
    /// Sibling files that must be downloaded alongside `model_file`.
    pub additional_files: Vec<String>,
    /// Maximum token sequence length.
    pub max_length: usize,
    /// Human-readable description.
    pub description: String,
}

/// Bundled SPLADE presets.
///
/// Self-hosted on `xberg-io/sparse-embeddings` (weights unmodified from their
/// Apache-2.0 upstreams); pinned via the checked-in `presets.sha256sum` manifest.
/// Both produce MLM-logit `[B, S, vocab]` output that the SPLADE engine max-pools
/// into a 30522-dim sparse vector.
/// SHA-256 manifest pinning every hosted sparse-embedding preset file, verified
/// at download time by [`crate::onnx::download_model_files`].
#[cfg(any(feature = "sparse-embeddings", test))]
pub(crate) const SPARSE_EMBEDDING_SHA256_MANIFEST: &str = include_str!("presets.sha256sum");

#[cfg(feature = "sparse-embeddings")]
const SPARSE_EMBEDDING_REVISION: &str = "5b5bccdae54b24d5a117a9e621775ebf97596d20";

pub static SPARSE_EMBEDDING_PRESETS: LazyLock<Vec<SparseEmbeddingPreset>> = LazyLock::new(|| {
    vec![
        SparseEmbeddingPreset {
            name: "splade".to_string(),
            model_repo: "xberg-io/sparse-embeddings".to_string(),
            model_file: "splade/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 256,
            description: "SPLADE++ EN v1 — English learned sparse retrieval (Apache-2.0).".to_string(),
        },
        SparseEmbeddingPreset {
            name: "opensearch-v3-distill".to_string(),
            model_repo: "xberg-io/sparse-embeddings".to_string(),
            model_file: "opensearch-v3-distill/model.onnx".to_string(),
            additional_files: vec!["opensearch-v3-distill/model.onnx.data".to_string()],
            max_length: 512,
            description: "OpenSearch neural-sparse v3 distill (DistilBERT MLM, 2026-gen, Apache-2.0). \
                Exported with its MLM head; 30522-dim SPLADE sparse vectors, 512 max-len."
                .to_string(),
        },
    ]
});

/// Look up a bundled SPLADE preset by exact name.
///
/// Since v5.0.0.
#[cfg(any(feature = "sparse-embedding-presets", feature = "sparse-embeddings"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_preset(name: &str) -> Option<SparseEmbeddingPreset> {
    SPARSE_EMBEDDING_PRESETS.iter().find(|p| p.name == name).cloned()
}

/// List the names of all bundled SPLADE presets.
///
/// Since v5.0.0.
#[cfg(any(feature = "sparse-embedding-presets", feature = "sparse-embeddings"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_presets() -> Vec<String> {
    SPARSE_EMBEDDING_PRESETS.iter().map(|p| p.name.clone()).collect()
}

#[cfg(feature = "sparse-embeddings")]
type CachedEngine = Arc<SparseEmbeddingEngine>;

#[cfg(feature = "sparse-embeddings")]
static ENGINE_CACHE: LazyLock<RwLock<AHashMap<String, CachedEngine>>> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Bounds concurrent blocking inference tasks spawned by [`embed_sparse_async`].
#[cfg(all(feature = "sparse-embeddings", feature = "tokio-runtime"))]
static SPARSE_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    let permits = crate::core::config::concurrency::resolve_batch_concurrency(None, true).max(1);
    Arc::new(tokio::sync::Semaphore::new(permits))
});

/// Module-tagged error constructor threaded into the shared onnx helpers.
#[cfg(feature = "sparse-embeddings")]
fn sparse_err(msg: String) -> crate::XbergError {
    crate::XbergError::embedding(msg)
}

/// Resolve `(repo, model_file, additional_files, max_length)` from a config model.
#[cfg(feature = "sparse-embeddings")]
fn resolve_model_info(
    model_type: &crate::core::config::SparseEmbeddingModelType,
    config_max_length: usize,
) -> crate::Result<(String, String, Vec<String>, usize)> {
    use crate::core::config::SparseEmbeddingModelType as M;
    match model_type {
        M::Preset { name } => {
            let preset =
                get_preset(name).ok_or_else(|| sparse_err(format!("Unknown sparse-embedding preset: {name}")))?;
            Ok((
                preset.model_repo,
                preset.model_file,
                preset.additional_files,
                preset.max_length,
            ))
        }
        M::Custom {
            model_id,
            model_file,
            additional_files,
            max_length,
        } => {
            let file = model_file.clone().unwrap_or_else(|| DEFAULT_MODEL_FILE.to_string());
            let max_len = match max_length {
                Some(v) if *v > 0 => *v as usize,
                _ => config_max_length,
            };
            Ok((model_id.clone(), file, additional_files.clone(), max_len))
        }
        M::Plugin { .. } => Err(sparse_err(
            "Plugin sparse-embedding backends are not yet supported; use Preset or Custom".to_string(),
        )),
    }
}

/// Get or initialize a sparse-embedding engine from cache.
#[cfg(feature = "sparse-embeddings")]
fn get_or_init_engine(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    max_length: usize,
    cache_dir: Option<std::path::PathBuf>,
    progress: crate::core::config::DownloadProgress,
    accel: Option<crate::core::config::acceleration::AccelerationConfig>,
) -> crate::Result<Arc<SparseEmbeddingEngine>> {
    let revision = (repo_name == "xberg-io/sparse-embeddings").then_some(SPARSE_EMBEDDING_REVISION);
    let cache_key = crate::model_download::hf_cache_key(cache_dir.as_deref());
    let engine_key = format!("{repo_name}_{model_file}_{}_{}", revision.unwrap_or("main"), cache_key);

    match ENGINE_CACHE.read() {
        Ok(cache) => {
            if let Some(cached) = cache.get(&engine_key) {
                return Ok(Arc::clone(cached));
            }
        }
        Err(poison) => {
            if let Some(cached) = poison.get_ref().get(&engine_key) {
                return Ok(Arc::clone(cached));
            }
        }
    }

    let mut cache = match ENGINE_CACHE.write() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
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
        Some(SPARSE_EMBEDDING_SHA256_MANIFEST),
        sparse_err,
    )?;
    let tokenizer = crate::onnx::load_tokenizer(&files, max_length, sparse_err)?;
    let session = crate::onnx::build_session(&files.model, accel.as_ref(), sparse_err)?;

    let engine = Arc::new(SparseEmbeddingEngine::new(tokenizer, session));
    cache.insert(engine_key, Arc::clone(&engine));
    Ok(engine)
}

#[cfg(feature = "sparse-embeddings")]
fn map_engine_err(e: engine::SparseEmbedError) -> crate::XbergError {
    use engine::SparseEmbedError as E;
    match e {
        E::Tokenizer(m) => sparse_err(format!("Tokenization failed: {m}")),
        E::Ort(err) => {
            let msg = err.to_string();
            if crate::onnx::looks_like_ort_error(&msg) {
                crate::XbergError::MissingDependency(format!(
                    "ONNX Runtime - {}",
                    crate::onnx::onnx_runtime_install_message()
                ))
            } else {
                sparse_err(format!("Sparse-embedding inference failed: {err}"))
            }
        }
        E::Shape(m) => sparse_err(format!("Unexpected model output shape: {m}")),
        E::NoOutput => sparse_err("Sparse-embedding model produced no output".to_string()),
    }
}

/// Generate sparse (SPLADE) embeddings for a batch of texts.
///
/// Returns one [`SparseEmbedding`] per input text, in order.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded/loaded, if ONNX Runtime is
/// unavailable, or if a `Plugin` model is selected (not yet supported).
///
/// Since v5.0.0.
#[cfg_attr(alef, alef(skip))]
#[cfg(feature = "sparse-embeddings")]
pub fn embed_sparse<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::SparseEmbeddingConfig,
) -> crate::Result<Vec<SparseEmbedding>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let (repo, model_file, additional, max_len) = resolve_model_info(&config.model, config.max_length)?;
    let engine = get_or_init_engine(
        &repo,
        &model_file,
        &additional,
        max_len,
        config.cache_dir.clone(),
        config.into(),
        config.acceleration.clone(),
    )?;

    engine.embed(texts, config.batch_size).map_err(map_engine_err)
}

/// Async wrapper over [`embed_sparse`]: runs the blocking ONNX inference on a
/// bounded blocking-task pool so it does not stall the async runtime.
///
/// Since v5.0.0.
#[cfg(all(feature = "sparse-embeddings", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_sparse_async(
    texts: Vec<String>,
    config: &crate::core::config::SparseEmbeddingConfig,
) -> crate::Result<Vec<SparseEmbedding>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let config = config.clone();
    let permit = SPARSE_SEMAPHORE
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| sparse_err(format!("Sparse-embedding semaphore closed: {e}")))?;

    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        embed_sparse(&texts, &config)
    })
    .await
    .map_err(|e| sparse_err(format!("Sparse-embedding task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fail-closed guarantee: every hosted sparse-embedding preset's weight file (and
    /// any external-data sibling) must be pinned in `presets.sha256sum`.
    #[test]
    fn every_preset_file_is_pinned_in_manifest() {
        let manifest = crate::model_download::parse_sha256_manifest(SPARSE_EMBEDDING_SHA256_MANIFEST).unwrap();
        let pinned: std::collections::HashSet<&str> = manifest.iter().map(|(p, _)| p.as_str()).collect();
        for preset in SPARSE_EMBEDDING_PRESETS.iter() {
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
    fn preset_catalog_is_nonempty_and_lookup_works() {
        assert!(!SPARSE_EMBEDDING_PRESETS.is_empty());
        assert!(list_presets().contains(&"splade".to_string()));
        let p = get_preset("splade").expect("splade preset present");
        assert_eq!(p.model_repo, "xberg-io/sparse-embeddings");
        assert_eq!(p.model_file, "splade/model.onnx");
        assert!(get_preset("does-not-exist").is_none());
    }

    #[test]
    fn sparse_embedding_serde_roundtrip() {
        let se = SparseEmbedding {
            indices: vec![3, 17, 200],
            values: vec![0.5, 0.25, 0.1],
        };
        let json = serde_json::to_string(&se).unwrap();
        let back: SparseEmbedding = serde_json::from_str(&json).unwrap();
        assert_eq!(se, back);
    }
}
