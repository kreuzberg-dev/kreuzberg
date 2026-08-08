//! ColBERT late-interaction (multi-vector) embeddings.
//!
//! Produces a *sequence* of per-token vectors per document (rather than a
//! single pooled vector) from a ColBERT-style ONNX model. Retrieval scores
//! documents against a query via MaxSim — for every query token, take the
//! maximum similarity against any document token, then sum across query
//! tokens — which captures fine-grained term-level interaction that dense
//! single-vector embeddings smear away.
//!
//! Built on the shared [`crate::onnx`] model-loading helpers. The engine math
//! (tokenization, marker insertion, query augmentation, per-token
//! normalization) is in [`engine`].
//!
//! Since v5.0.0.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

#[cfg(feature = "late-interaction")]
pub mod engine;

#[cfg(feature = "late-interaction")]
use std::sync::{Arc, RwLock};

#[cfg(feature = "late-interaction")]
use ahash::AHashMap;
#[cfg(feature = "late-interaction")]
use engine::LateInteractionEngine;

/// Default ONNX file for a `Custom` ColBERT repo when none is specified.
#[cfg(feature = "late-interaction")]
const DEFAULT_MODEL_FILE: &str = "onnx/model.onnx";

/// A ColBERT multi-vector embedding: one row per attention-live token.
///
/// `data` is a flat, row-major buffer of length `num_tokens * dim` — row `i`
/// (the embedding for token `i`) occupies `data[i*dim .. (i+1)*dim]`. Flat
/// storage keeps the type FFI-friendly across binding boundaries; use
/// [`MultiVectorEmbedding::rows`] internally to iterate per-token slices.
///
/// Since v5.0.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct MultiVectorEmbedding {
    /// Number of attention-live token rows (padding rows are dropped, not
    /// zeroed — see [`engine::normalize_tokens`]).
    pub num_tokens: u32,
    /// Dimensionality of each per-token vector.
    pub dim: u32,
    /// Flat row-major buffer, length `num_tokens * dim`.
    pub data: Vec<f32>,
}

impl MultiVectorEmbedding {
    /// Returns `true` if `data` holds exactly `num_tokens * dim` values — i.e.
    /// the flat buffer matches the declared shape.
    ///
    /// All fields are `pub` and the type is `Deserialize`, so a value coming
    /// from an untrusted source (FFI caller, JSON, a store row) may be
    /// malformed. [`max_sim_score`] guards on this so a length-mismatched
    /// buffer scores `0.0` rather than silently mis-scoring (a shorter `data`
    /// would make [`rows`](Self::rows)' `chunks_exact` drop a trailing partial
    /// chunk). Uses `checked_mul` so an overflowing `num_tokens * dim` is
    /// reported as malformed instead of wrapping.
    ///
    /// Since v5.0.0.
    pub fn is_well_formed(&self) -> bool {
        (self.num_tokens as usize)
            .checked_mul(self.dim as usize)
            .is_some_and(|expected| expected == self.data.len())
    }

    /// Iterate over this embedding's per-token vectors as `&[f32]` slices.
    ///
    /// Internal helper for MaxSim scoring; not part of the FFI-facing surface.
    /// Panic-safe on `dim == 0` (`chunks_exact(0)` would panic): a zero-dim
    /// value yields an empty iterator. Callers should still validate via
    /// [`Self::is_well_formed`] to get a meaningful row count.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &[f32]> {
        let dim = self.dim as usize;
        let (data, step): (&[f32], usize) = if dim == 0 { (&[], 1) } else { (&self.data, dim) };
        data.chunks_exact(step)
    }
}

/// Static metadata for a bundled ColBERT preset (WASM/Android-safe, no ORT).
///
/// Since v5.0.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LateInteractionPreset {
    /// Stable preset name referenced from config.
    pub name: String,
    /// HuggingFace repository hosting the ONNX model.
    pub model_repo: String,
    /// Path to the ONNX file within the repo.
    pub model_file: String,
    /// Sibling files that must be downloaded alongside `model_file`.
    pub additional_files: Vec<String>,
    /// Maximum document token sequence length.
    pub max_length: usize,
    /// Fixed padded query length (ColBERT query augmentation).
    pub query_max_length: usize,
    /// Per-token embedding dimensionality.
    pub dim: usize,
    /// Human-readable description.
    pub description: String,
}

/// Bundled ColBERT presets.
///
/// Self-hosted on `xberg-io/late-interaction-models` (mirror of the Apache-2.0
/// `answerdotai/answerai-colbert-small-v1`, weights unmodified); pinned via the
/// checked-in `presets.sha256sum` manifest.
/// SHA-256 manifest pinning every hosted late-interaction preset file, verified
/// at download time by [`crate::onnx::download_model_files`].
#[cfg(any(feature = "late-interaction", test))]
pub(crate) const LATE_INTERACTION_SHA256_MANIFEST: &str = include_str!("presets.sha256sum");

#[cfg(feature = "late-interaction")]
const LATE_INTERACTION_REVISION: &str = "bcaafd6b4e8f0e1da252661f80ca894c714d1f62";

pub static LATE_INTERACTION_PRESETS: LazyLock<Vec<LateInteractionPreset>> = LazyLock::new(|| {
    vec![
        LateInteractionPreset {
            name: "colbert".to_string(),
            model_repo: "xberg-io/late-interaction-models".to_string(),
            model_file: "colbert-small-v1/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 512,
            query_max_length: 32,
            dim: 96,
            description: "AnswerAI ColBERT small v1 — English multi-vector late-interaction retrieval (Apache-2.0)."
                .to_string(),
        },
        LateInteractionPreset {
            name: "gte-moderncolbert".to_string(),
            model_repo: "xberg-io/late-interaction-models".to_string(),
            model_file: "gte-moderncolbert-v1/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 512,
            query_max_length: 32,
            dim: 128,
            description: "LightOn GTE-ModernColBERT v1 — ModernBERT long-context multi-vector \
                late-interaction retrieval, 128-dim tokens (Apache-2.0)."
                .to_string(),
        },
    ]
});

/// Look up a bundled ColBERT preset by exact name.
///
/// Since v5.0.0.
#[cfg(any(feature = "late-interaction-presets", feature = "late-interaction"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_preset(name: &str) -> Option<LateInteractionPreset> {
    LATE_INTERACTION_PRESETS.iter().find(|p| p.name == name).cloned()
}

/// List the names of all bundled ColBERT presets.
///
/// Since v5.0.0.
#[cfg(any(feature = "late-interaction-presets", feature = "late-interaction"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_presets() -> Vec<String> {
    LATE_INTERACTION_PRESETS.iter().map(|p| p.name.clone()).collect()
}

/// A single document match returned by [`max_sim_rank`], with its position in
/// the input and MaxSim score.
///
/// Since v5.0.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LateInteractionMatch {
    /// Position of this document in the original input slice.
    pub index: usize,
    /// MaxSim relevance score. Higher means more relevant to the query.
    pub score: f32,
}

/// Score a query against a document using ColBERT's MaxSim operator: for each
/// query token vector, take the maximum dot product against any document
/// token vector, then sum across query tokens.
///
/// Returns `0.0` if `query` and `doc` have mismatched dimensionality, if either
/// has zero tokens, or if either is not well-formed per
/// [`MultiVectorEmbedding::is_well_formed`] (its `data` length does not match
/// `num_tokens * dim`).
///
/// Pure CPU primitive — available without ONNX Runtime.
///
/// Since v5.0.0.
#[cfg(any(feature = "late-interaction-presets", feature = "late-interaction"))]
pub fn max_sim_score(query: &MultiVectorEmbedding, doc: &MultiVectorEmbedding) -> f64 {
    if query.dim != doc.dim
        || query.dim == 0
        || query.num_tokens == 0
        || doc.num_tokens == 0
        || !query.is_well_formed()
        || !doc.is_well_formed()
    {
        return 0.0;
    }

    query
        .rows()
        .map(|q_row| {
            doc.rows()
                .map(|d_row| q_row.iter().zip(d_row.iter()).map(|(a, b)| a * b).sum::<f32>())
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .sum::<f32>() as f64
}

/// Rank a set of documents against a query by MaxSim score, descending.
///
/// Mirrors the sort/truncate shape of `crate::reranking`'s `build_results`,
/// minus top-k truncation (callers slice the returned `Vec` themselves).
///
/// Pure CPU primitive — available without ONNX Runtime.
///
/// Since v5.0.0.
#[cfg(any(feature = "late-interaction-presets", feature = "late-interaction"))]
pub fn max_sim_rank(query: &MultiVectorEmbedding, docs: &[MultiVectorEmbedding]) -> Vec<LateInteractionMatch> {
    let mut results: Vec<LateInteractionMatch> = docs
        .iter()
        .enumerate()
        .map(|(index, doc)| LateInteractionMatch {
            index,
            score: max_sim_score(query, doc) as f32,
        })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results
}

#[cfg(feature = "late-interaction")]
type CachedEngine = Arc<LateInteractionEngine>;

#[cfg(feature = "late-interaction")]
static ENGINE_CACHE: LazyLock<RwLock<AHashMap<String, CachedEngine>>> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Bounds concurrent blocking inference tasks spawned by [`embed_multi_vector_async`].
#[cfg(all(feature = "late-interaction", feature = "tokio-runtime"))]
static LATE_INTERACTION_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    let permits = crate::core::config::concurrency::resolve_batch_concurrency(None, true).max(1);
    Arc::new(tokio::sync::Semaphore::new(permits))
});

/// Module-tagged error constructor threaded into the shared onnx helpers.
#[cfg(feature = "late-interaction")]
fn late_err(msg: String) -> crate::XbergError {
    crate::XbergError::embedding(msg)
}

/// Resolve `(repo, model_file, additional_files, max_length, query_max_length)`
/// from a config model.
#[cfg(feature = "late-interaction")]
fn resolve_model_info(
    model_type: &crate::core::config::LateInteractionModelType,
    config_max_length: usize,
    config_query_max_length: usize,
) -> crate::Result<(String, String, Vec<String>, usize, usize)> {
    use crate::core::config::LateInteractionModelType as M;
    match model_type {
        M::Preset { name } => {
            let preset =
                get_preset(name).ok_or_else(|| late_err(format!("Unknown late-interaction preset: {name}")))?;
            Ok((
                preset.model_repo,
                preset.model_file,
                preset.additional_files,
                preset.max_length,
                preset.query_max_length,
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
            Ok((
                model_id.clone(),
                file,
                additional_files.clone(),
                max_len,
                config_query_max_length,
            ))
        }
        M::Plugin { .. } => Err(late_err(
            "Plugin late-interaction backends are not yet supported; use Preset or Custom".to_string(),
        )),
    }
}

/// Get or initialize a late-interaction engine from cache.
#[cfg(feature = "late-interaction")]
#[allow(clippy::too_many_arguments)]
fn get_or_init_engine(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    max_length: usize,
    query_max_length: usize,
    cache_dir: Option<std::path::PathBuf>,
    progress: crate::core::config::DownloadProgress,
    accel: Option<crate::core::config::acceleration::AccelerationConfig>,
) -> crate::Result<Arc<LateInteractionEngine>> {
    let revision = (repo_name == "xberg-io/late-interaction-models").then_some(LATE_INTERACTION_REVISION);
    let cache_key = crate::model_download::hf_cache_key(cache_dir.as_deref());
    let engine_key = format!(
        "{repo_name}_{model_file}_{}_{}_{}",
        revision.unwrap_or("main"),
        cache_key,
        query_max_length
    );

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
        Some(LATE_INTERACTION_SHA256_MANIFEST),
        late_err,
    )?;
    let tokenizer = crate::onnx::load_tokenizer(&files, max_length, late_err)?;
    let session = crate::onnx::build_session(&files.model, accel.as_ref(), late_err)?;

    let query_marker_id = tokenizer.token_to_id("[Q]");
    let doc_marker_id = tokenizer.token_to_id("[D]");
    let mask_id = tokenizer.token_to_id("[MASK]");

    let engine = Arc::new(LateInteractionEngine::new(
        tokenizer,
        session,
        query_marker_id,
        doc_marker_id,
        mask_id,
        query_max_length,
    ));
    cache.insert(engine_key, Arc::clone(&engine));
    Ok(engine)
}

#[cfg(feature = "late-interaction")]
fn map_engine_err(e: engine::LateInteractionError) -> crate::XbergError {
    use engine::LateInteractionError as E;
    match e {
        E::Tokenizer(m) => late_err(format!("Tokenization failed: {m}")),
        E::Ort(err) => {
            let msg = err.to_string();
            if crate::onnx::looks_like_ort_error(&msg) {
                crate::XbergError::MissingDependency(format!(
                    "ONNX Runtime - {}",
                    crate::onnx::onnx_runtime_install_message()
                ))
            } else {
                late_err(format!("Late-interaction inference failed: {err}"))
            }
        }
        E::Shape(m) => late_err(format!("Unexpected model output shape: {m}")),
        E::NoOutput => late_err("Late-interaction model produced no output".to_string()),
    }
}

/// Generate ColBERT multi-vector embeddings for a batch of texts.
///
/// `is_query` selects marker-token insertion (query `[Q]` vs. document `[D]`)
/// and, when `true`, applies fixed-length query augmentation padding.
///
/// Returns one [`MultiVectorEmbedding`] per input text, in order.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded/loaded, if ONNX Runtime
/// is unavailable, or if a `Plugin` model is selected (not yet supported).
///
/// Since v5.0.0.
#[cfg_attr(alef, alef(skip))]
#[cfg(feature = "late-interaction")]
pub fn embed_multi_vector<T: AsRef<str>>(
    texts: &[T],
    config: &crate::core::config::LateInteractionConfig,
    is_query: bool,
) -> crate::Result<Vec<MultiVectorEmbedding>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let (repo, model_file, additional, max_len, query_max_len) =
        resolve_model_info(&config.model, config.max_length, config.query_max_length)?;
    let engine = get_or_init_engine(
        &repo,
        &model_file,
        &additional,
        max_len,
        query_max_len,
        config.cache_dir.clone(),
        config.into(),
        config.acceleration.clone(),
    )?;

    engine.embed(texts, config.batch_size, is_query).map_err(map_engine_err)
}

/// Async wrapper over [`embed_multi_vector`]: runs the blocking ONNX inference
/// on a bounded blocking-task pool so it does not stall the async runtime.
///
/// Since v5.0.0.
#[cfg(all(feature = "late-interaction", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_multi_vector_async(
    texts: Vec<String>,
    config: &crate::core::config::LateInteractionConfig,
    is_query: bool,
) -> crate::Result<Vec<MultiVectorEmbedding>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let config = config.clone();
    let permit = LATE_INTERACTION_SEMAPHORE
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| late_err(format!("Late-interaction semaphore closed: {e}")))?;

    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        embed_multi_vector(&texts, &config, is_query)
    })
    .await
    .map_err(|e| late_err(format!("Late-interaction task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fail-closed guarantee: every hosted late-interaction preset's weight file (and
    /// any external-data sibling) must be pinned in `presets.sha256sum`.
    #[test]
    fn every_preset_file_is_pinned_in_manifest() {
        let manifest = crate::model_download::parse_sha256_manifest(LATE_INTERACTION_SHA256_MANIFEST).unwrap();
        let pinned: std::collections::HashSet<&str> = manifest.iter().map(|(p, _)| p.as_str()).collect();
        for preset in LATE_INTERACTION_PRESETS.iter() {
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
        assert!(!LATE_INTERACTION_PRESETS.is_empty());
        assert!(list_presets().contains(&"colbert".to_string()));
        let p = get_preset("colbert").expect("colbert preset present");
        assert_eq!(p.model_repo, "xberg-io/late-interaction-models");
        assert_eq!(p.model_file, "colbert-small-v1/model.onnx");
        assert_eq!(p.query_max_length, 32);
        assert!(get_preset("does-not-exist").is_none());
    }

    #[test]
    fn multi_vector_embedding_serde_roundtrip() {
        let mv = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 3,
            data: vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
        };
        let json = serde_json::to_string(&mv).unwrap();
        let back: MultiVectorEmbedding = serde_json::from_str(&json).unwrap();
        assert_eq!(mv, back);
    }

    #[test]
    fn rows_iterates_token_vectors() {
        let mv = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 2,
            data: vec![1.0, 2.0, 3.0, 4.0],
        };
        let rows: Vec<&[f32]> = mv.rows().collect();
        assert_eq!(rows, vec![[1.0, 2.0].as_slice(), [3.0, 4.0].as_slice()]);
    }

    #[test]
    fn max_sim_score_sums_best_match_per_query_token() {
        let query = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 2,
            data: vec![1.0, 0.0, 0.0, 1.0],
        };
        let doc = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 2,
            data: vec![1.0, 0.0, 0.6, 0.8],
        };
        let score = max_sim_score(&query, &doc);
        assert!((score - 1.8).abs() < 1e-5, "expected 1.8, got {score}");
    }

    #[test]
    fn max_sim_score_zero_on_dim_mismatch() {
        let query = MultiVectorEmbedding {
            num_tokens: 1,
            dim: 2,
            data: vec![1.0, 0.0],
        };
        let doc = MultiVectorEmbedding {
            num_tokens: 1,
            dim: 3,
            data: vec![1.0, 0.0, 0.0],
        };
        assert_eq!(max_sim_score(&query, &doc), 0.0);
    }

    #[test]
    fn max_sim_score_zero_on_empty_tokens() {
        let query = MultiVectorEmbedding {
            num_tokens: 0,
            dim: 2,
            data: vec![],
        };
        let doc = MultiVectorEmbedding {
            num_tokens: 1,
            dim: 2,
            data: vec![1.0, 0.0],
        };
        assert_eq!(max_sim_score(&query, &doc), 0.0);
    }

    #[test]
    fn is_well_formed_checks_data_length_against_shape() {
        let ok = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 3,
            data: vec![0.0; 6],
        };
        assert!(ok.is_well_formed());

        let short = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 3,
            data: vec![0.0; 5],
        };
        assert!(!short.is_well_formed());

        let overflow = MultiVectorEmbedding {
            num_tokens: u32::MAX,
            dim: u32::MAX,
            data: vec![0.0; 1],
        };
        assert!(!overflow.is_well_formed());
    }

    #[test]
    fn max_sim_score_zero_on_malformed_buffer() {
        let query = MultiVectorEmbedding {
            num_tokens: 1,
            dim: 2,
            data: vec![1.0, 0.0],
        };
        let malformed_doc = MultiVectorEmbedding {
            num_tokens: 2,
            dim: 2,
            data: vec![1.0, 0.0, 0.6],
        };
        assert_eq!(max_sim_score(&query, &malformed_doc), 0.0);
    }

    #[test]
    fn max_sim_rank_sorts_descending_by_score() {
        let query = MultiVectorEmbedding {
            num_tokens: 1,
            dim: 2,
            data: vec![1.0, 0.0],
        };
        let docs = vec![
            MultiVectorEmbedding {
                num_tokens: 1,
                dim: 2,
                data: vec![0.5, 0.0],
            },
            MultiVectorEmbedding {
                num_tokens: 1,
                dim: 2,
                data: vec![1.0, 0.0],
            },
            MultiVectorEmbedding {
                num_tokens: 1,
                dim: 2,
                data: vec![0.1, 0.0],
            },
        ];
        let ranked = max_sim_rank(&query, &docs);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].index, 1);
        assert_eq!(ranked[1].index, 0);
        assert_eq!(ranked[2].index, 2);
        assert!(ranked[0].score >= ranked[1].score);
        assert!(ranked[1].score >= ranked[2].score);
    }
}
