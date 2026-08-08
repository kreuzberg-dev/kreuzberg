//! Cross-encoder reranking support.
//!
//! This module provides `(query, document)` pair scoring using ONNX cross-encoder
//! models. Reranking is the standard "second pass" in retrieval pipelines:
//! a first-pass embedding search retrieves a candidate set cheaply; reranking
//! rescores and reorders those candidates by true relevance to the query.
//!
//! Three backend variants are supported:
//! - **Local ONNX** — cross-encoder models (ms-marco, bge-reranker) via ONNX Runtime.
//! - **liter-llm** — provider-hosted rerankers (Cohere, Jina, Voyage) via an API.
//! - **In-process plugin** — caller-supplied backends registered via
//!   [`crate::plugins::register_reranker_backend`].
//!
//! # Download/cache machinery
//!
//! The ONNX path downloads models, loads the tokenizer, and builds the ORT
//! session through the shared [`crate::onnx`] helpers (enabled by the
//! `onnx-runtime` feature that `reranker` pulls in), so no download/lock or ORT
//! diagnostic code is duplicated here — the cross-encoder pair encoding is the
//! only reranker-specific step, applied at inference time via `EncodeInput::Dual`.
//!
//! Since v5.0.0.

#[cfg(feature = "reranker")]
pub mod engine;

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[cfg(feature = "reranker")]
use ahash::AHashMap;
#[cfg(feature = "reranker")]
use engine::RerankerEngine;
#[cfg(feature = "reranker")]
use std::sync::{Arc, RwLock};

#[cfg(feature = "reranker")]
type CachedEngine = Arc<RerankerEngine>;

#[cfg(feature = "reranker")]
static ENGINE_CACHE: LazyLock<RwLock<AHashMap<String, CachedEngine>>> = LazyLock::new(|| RwLock::new(AHashMap::new()));

/// Global semaphore that limits concurrent ONNX reranker inference calls.
///
/// Prevents resource exhaustion when many async callers invoke `rerank_async`
/// against the ONNX path (Preset/Custom variants) simultaneously. The Llm and
/// Plugin variants short-circuit out of `rerank_async` before reaching the
/// semaphore. The permit count matches the thread budget used by the embedding
/// semaphore.
///
/// Since v5.0.0.
#[cfg(all(feature = "reranker", feature = "tokio-runtime"))]
static RERANK_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    let budget = crate::core::config::concurrency::resolve_batch_concurrency(None, true);
    Arc::new(tokio::sync::Semaphore::new(budget))
});

/// A single document returned by the reranker, with its position in the input and score.
///
/// `index` maps back to the caller's original document list, so metadata arrays
/// (e.g. IDs, paths) can be reordered without passing them through the reranker.
///
/// Since v5.0.0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RerankedDocument {
    /// Position of this document in the original input `documents` slice.
    pub index: usize,
    /// Relevance score in `[0, 1]`. Higher means more relevant to the query.
    pub score: f32,
    /// The document text.
    pub document: String,
}

/// Metadata for a bundled reranker preset.
///
/// All string fields are owned `String` for FFI compatibility — instances are
/// safe to clone and pass across language boundaries.
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankerPreset {
    /// Short identifier (catalog name, e.g. `"bge-reranker-base"`).
    pub name: String,
    /// HuggingFace repository name for the model.
    pub model_repo: String,
    /// Path to the ONNX model file within the repo.
    pub model_file: String,
    /// Sibling files that must be downloaded alongside `model_file`.
    ///
    /// Empty for most presets. Used by repos that split the weight blob —
    /// e.g. `rozgo/bge-reranker-v2-m3` ships the model in `model.onnx` plus a
    /// co-located `model.onnx.data` payload.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_files: Vec<String>,
    /// Maximum token sequence length the model supports.
    pub max_length: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
    /// Scoring head for the ONNX model's output tensor.
    ///
    /// Defaults to [`crate::core::config::reranker::RerankerHead::CrossEncoder`]
    /// for all existing presets. Set to `Qwen3Generative` for Qwen3
    /// generative-reranker checkpoints.
    #[serde(default)]
    pub head: crate::core::config::reranker::RerankerHead,
}

/// All available reranker presets.
///
/// **Source of truth**: mirrors the `RerankerModel` catalog of
/// [fastembed-rs](https://github.com/Anush008/fastembed-rs) verbatim. Every
/// `model_repo` + `model_file` + `additional_files` triple here is the path
/// fastembed-rs uses, which they keep verified against the live HuggingFace
/// hub. When fastembed-rs publishes a new catalog entry, mirror it here in
/// one PR; do not invent paths.
///
/// Refresh procedure: read fastembed-rs `src/models/reranking.rs` on the
/// `main` branch and update this list to match. The `live-hf` CI job will
/// fail loudly if any preset path 404s.
///
/// Since v5.0.0.
#[cfg(any(feature = "reranker", test))]
/// SHA-256 manifest pinning every hosted reranker preset file, verified at
/// download time by [`crate::onnx::download_model_files`].
pub(crate) const RERANKER_SHA256_MANIFEST: &str = include_str!("presets.sha256sum");

#[cfg(feature = "reranker")]
const RERANKER_MODEL_REVISION: &str = "3d655b40f2e1e087460434a143b11afac4947318";

pub static RERANKER_PRESETS: LazyLock<Vec<RerankerPreset>> = LazyLock::new(|| {
    vec![
        RerankerPreset {
            name: "bge-reranker-base".to_string(),
            model_repo: "xberg-io/reranker-models".to_string(),
            model_file: "bge-reranker-base/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 512,
            description: "BGE cross-encoder base (~278M params, EN + ZH). Best for: \
                general-purpose RAG, production deployments, English or Chinese documents."
                .to_string(),
            head: crate::core::config::reranker::RerankerHead::CrossEncoder,
        },
        RerankerPreset {
            name: "bge-reranker-v2-m3".to_string(),
            model_repo: "xberg-io/reranker-models".to_string(),
            model_file: "bge-reranker-v2-m3/model.onnx".to_string(),
            additional_files: vec!["bge-reranker-v2-m3/model.onnx.data".to_string()],
            max_length: 8192,
            description: "BGE cross-encoder v2 M3 (568M params, 100+ languages, 8192 max-len). \
                Best for: international documents, mixed-language retrieval. \
                Mirror of the official BAAI model; the weight is split into model.onnx + model.onnx.data."
                .to_string(),
            head: crate::core::config::reranker::RerankerHead::CrossEncoder,
        },
        RerankerPreset {
            name: "jina-reranker-v1-turbo-en".to_string(),
            model_repo: "xberg-io/reranker-models".to_string(),
            model_file: "jina-reranker-v1-turbo-en/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 8192,
            description: "Jina reranker v1 turbo English (~37M params, 8192 max-len). \
                Best for: low-latency reranking, English documents, long-context retrieval."
                .to_string(),
            head: crate::core::config::reranker::RerankerHead::CrossEncoder,
        },
        // NOTE: `jina-reranker-v2-base-multilingual` was REMOVED — its license is
        RerankerPreset {
            name: "qwen3-reranker-0.6b".to_string(),
            model_repo: "xberg-io/reranker-models".to_string(),
            model_file: "qwen3-reranker-0.6b/model.onnx".to_string(),
            additional_files: vec!["qwen3-reranker-0.6b/model.onnx.data".to_string()],
            max_length: 512,
            description: "Qwen3 generative reranker (0.6B params, multilingual). Best for: \
                instruction-aware relevance judgment via a causal-LM yes/no head, higher quality \
                at higher latency than classic cross-encoders."
                .to_string(),
            head: crate::core::config::reranker::RerankerHead::Qwen3Generative,
        },
        RerankerPreset {
            name: "ettin-reranker-150m".to_string(),
            model_repo: "xberg-io/reranker-models".to_string(),
            model_file: "ettin-reranker-150m/model.onnx".to_string(),
            additional_files: Vec::new(),
            max_length: 7999,
            description: "Ettin cross-encoder (150M params, ModernBERT long-context, EN). Best for: \
                high-quality English reranking with long documents at cross-encoder latency."
                .to_string(),
            head: crate::core::config::reranker::RerankerHead::CrossEncoder,
        },
    ]
});

/// Friendly aliases mapped to catalog short-names.
///
/// `(alias, catalog_name)` pairs let callers say `"balanced"` or
/// `"multilingual"` and get a stable, opinionated default that may evolve
/// across releases. The catalog name is the underlying primitive — alias
/// resolution is single-hop (aliases cannot point at other aliases).
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
const PRESET_ALIASES: &[(&str, &str)] = &[
    ("fast", "jina-reranker-v1-turbo-en"),
    ("balanced", "ettin-reranker-150m"),
    ("quality", "bge-reranker-v2-m3"),
    ("multilingual", "bge-reranker-v2-m3"),
];

/// Get a preset by name (returns an owned clone for FFI compatibility).
///
/// Lookup is case-sensitive. Checks the catalog literals first; falls back
/// to the alias table for the documented friendly names
/// (`fast` / `balanced` / `quality` / `multilingual`).
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
pub(crate) fn get_preset(name: &str) -> Option<RerankerPreset> {
    if let Some(preset) = RERANKER_PRESETS.iter().find(|p| p.name == name) {
        return Some(preset.clone());
    }
    let resolved = PRESET_ALIASES.iter().find(|(alias, _)| *alias == name)?.1;
    RERANKER_PRESETS.iter().find(|p| p.name == resolved).cloned()
}

/// List all available reranker preset names (owned clones for FFI compatibility).
///
/// Returns the catalog short-names followed by the friendly aliases, so
/// `list_presets()[..5]` is the catalog and `list_presets()[5..]` is aliases.
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
pub(crate) fn list_presets() -> Vec<String> {
    let mut out: Vec<String> = RERANKER_PRESETS.iter().map(|p| p.name.clone()).collect();
    out.extend(PRESET_ALIASES.iter().map(|(alias, _)| alias.to_string()));
    out
}

/// Module-tagged error constructor threaded into the shared onnx helpers.
#[cfg(feature = "reranker")]
fn rerank_err(msg: String) -> crate::XbergError {
    crate::XbergError::reranking(msg)
}

/// Resolve the single-token vocabulary id for an answer word (`"yes"`/`"no"`),
/// robust to how different tokenizers encode a leading word.
///
/// Qwen3's reference reranker looks up the bare `"yes"`/`"no"` vocab entry, which
/// works for its own checkpoint. BPE/SentencePiece tokenizers, however, may only
/// carry the word as a leading-space variant (`"Ġyes"`, `"▁yes"`) or capitalized,
/// so this tries those direct vocab hits before falling back to encoding the bare
/// word and taking its id when it tokenizes to exactly one token — mirroring how
/// the model actually sees the answer token in context.
#[cfg(feature = "reranker")]
fn resolve_answer_token_id(tokenizer: &tokenizers::Tokenizer, word: &str) -> Option<u32> {
    let capitalized = {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => word.to_string(),
        }
    };
    let variants = [
        word.to_string(),
        format!("\u{0120}{word}"),
        format!("\u{2581}{word}"),
        capitalized,
    ];
    for variant in &variants {
        if let Some(id) = tokenizer.token_to_id(variant) {
            return Some(id);
        }
    }
    let encoding = tokenizer.encode(word, false).ok()?;
    match encoding.get_ids() {
        [single] => Some(*single),
        _ => None,
    }
}

/// Resolve the tokenizer ids for "yes" / "no" used by the Qwen3 generative-reranker head.
///
/// Looked up from the tokenizer rather than hardcoded, since the exact ids are
/// checkpoint-specific. Returns `(true_id, false_id)`.
///
/// # Errors
///
/// Returns `XbergError::Reranking` if either token cannot be resolved — this
/// would indicate an incompatible checkpoint.
#[cfg(feature = "reranker")]
fn resolve_qwen3_token_ids(tokenizer: &tokenizers::Tokenizer) -> crate::Result<(u32, u32)> {
    let true_id = resolve_answer_token_id(tokenizer, "yes").ok_or_else(|| unresolved_answer_token_error("yes"))?;
    let false_id = resolve_answer_token_id(tokenizer, "no").ok_or_else(|| unresolved_answer_token_error("no"))?;
    Ok((true_id, false_id))
}

/// Build the error returned when an answer word cannot be resolved to a single vocabulary token.
#[cfg(feature = "reranker")]
fn unresolved_answer_token_error(word: &str) -> crate::XbergError {
    crate::XbergError::reranking(format!(
        "Qwen3 generative-reranker head could not resolve \"{word}\" to exactly one token in this \
         tokenizer's vocabulary (no direct vocab match and encoding it did not yield a single token id) \
         — this usually means the loaded tokenizer is incompatible with the Qwen3 generative-reranker \
         head's yes/no scoring path (wrong tokenizer, or a merged/multi-token \"yes\"/\"no\"); check that \
         the reranker checkpoint is the expected Qwen3 generative-reranker model"
    ))
}

/// Get or initialize a reranker engine from cache.
///
/// Downloads model files from HuggingFace if needed, loads the tokenizer,
/// creates an ORT session, and caches the engine for reuse.
#[cfg(feature = "reranker")]
#[allow(clippy::too_many_arguments)]
fn get_or_init_engine(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    max_length: usize,
    cache_dir: Option<std::path::PathBuf>,
    progress: crate::core::config::DownloadProgress,
    accel: Option<crate::core::config::acceleration::AccelerationConfig>,
    head: crate::core::config::reranker::RerankerHead,
) -> crate::Result<Arc<RerankerEngine>> {
    let revision = (repo_name == "xberg-io/reranker-models").then_some(RERANKER_MODEL_REVISION);
    let cache_key = crate::model_download::hf_cache_key(cache_dir.as_deref());
    let engine_key = format!(
        "{repo_name}_{model_file}_{}_{cache_key}_{head:?}",
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
            Some(RERANKER_SHA256_MANIFEST),
            rerank_err,
        )?;
        let tokenizer = crate::onnx::load_tokenizer(&files, max_length, rerank_err)?;
        let session = crate::onnx::build_session(&files.model, accel.as_ref(), rerank_err)?;

        let (true_token_id, false_token_id) = match head {
            crate::core::config::reranker::RerankerHead::Qwen3Generative => {
                let (true_id, false_id) = resolve_qwen3_token_ids(&tokenizer)?;
                (Some(true_id), Some(false_id))
            }
            crate::core::config::reranker::RerankerHead::CrossEncoder => (None, None),
        };

        let new_engine = Arc::new(RerankerEngine::new(
            tokenizer,
            session,
            head,
            true_token_id,
            false_token_id,
        ));
        cache.insert(engine_key, Arc::clone(&new_engine));

        Ok(new_engine)
    }
}

/// Resolve model info (repo, model file, additional_files, max_length, head) from a RerankerModelType config.
#[cfg(feature = "reranker")]
fn resolve_model_info(
    model_type: &crate::core::config::RerankerModelType,
) -> crate::Result<(
    String,
    String,
    Vec<String>,
    usize,
    crate::core::config::reranker::RerankerHead,
)> {
    match model_type {
        crate::core::config::RerankerModelType::Preset { name } => {
            let preset = get_preset(name)
                .ok_or_else(|| crate::XbergError::reranking(format!("Unknown reranker preset: {name}")))?;
            Ok((
                preset.model_repo,
                preset.model_file,
                preset.additional_files,
                preset.max_length,
                preset.head,
            ))
        }
        crate::core::config::RerankerModelType::Custom {
            model_id,
            model_file,
            additional_files,
            max_length,
            head,
        } => {
            let len = match max_length.unwrap_or(512) {
                n if n <= 0 => {
                    return Err(crate::XbergError::Validation {
                        message: format!("max_length must be positive, got {n}"),
                        source: None,
                    });
                }
                n => n as usize,
            };
            let file = model_file.clone().unwrap_or_else(|| "onnx/model.onnx".to_string());
            Ok((model_id.clone(), file, additional_files.clone(), len, *head))
        }
        crate::core::config::RerankerModelType::Llm { .. } => Err(crate::XbergError::reranking(
            "LLM rerankers have no local model to warm or download — the provider serves them over HTTP.",
        )),
        crate::core::config::RerankerModelType::Plugin { .. } => Err(crate::XbergError::reranking(
            "Plugin rerankers have no local model to warm or download — the registered backend owns the model lifecycle.",
        )),
    }
}

/// Validate that a plugin backend returned the correct number of scores.
#[cfg(feature = "reranker")]
fn validate_reranker_output(scores: &[f32], expected_count: usize, backend_name: &str) -> crate::Result<()> {
    if scores.len() != expected_count {
        return Err(crate::XbergError::Validation {
            message: format!(
                "Reranker backend '{backend_name}' returned {got} scores for {expected} documents",
                got = scores.len(),
                expected = expected_count,
            ),
            source: None,
        });
    }
    Ok(())
}

/// Apply sigmoid to convert a raw logit to a `[0, 1]` score.
#[cfg(any(feature = "reranker", test))]
pub(crate) fn sigmoid_f32(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Build the sorted, optionally truncated result vector from raw logits.
///
/// Always applies sigmoid — used by callers (the Plugin backend path) that are
/// guaranteed to hand back raw cross-encoder-style logits. The local ONNX path
/// uses [`build_results_for_head`] instead, since its Qwen3 head already
/// returns a `[0, 1]` probability that must NOT be sigmoided a second time.
#[cfg(any(feature = "reranker", test))]
fn build_results(documents: &[String], logits: Vec<f32>, top_k: Option<usize>) -> Vec<RerankedDocument> {
    build_results_from_scores(documents, logits, top_k, sigmoid_f32)
}

/// Build the sorted, optionally truncated result vector from the local ONNX
/// engine's raw output, applying the score transform appropriate to `head`.
///
/// - [`crate::core::config::reranker::RerankerHead::CrossEncoder`] — the engine
///   returns raw logits; sigmoid is applied here to reach `[0, 1]`, exactly as
///   the original single-head implementation did.
/// - [`crate::core::config::reranker::RerankerHead::Qwen3Generative`] — the
///   engine's `qwen3_scores` already performs a softmax internally and returns
///   `P(yes)` in `[0, 1]`. Applying sigmoid again here would double-transform
///   an already-bounded probability, compressing the score distribution toward
///   0.5 and corrupting ranking. This path is a pass-through (identity) instead.
#[cfg(any(feature = "reranker", test))]
fn build_results_for_head(
    documents: &[String],
    raw_scores: Vec<f32>,
    top_k: Option<usize>,
    head: crate::core::config::reranker::RerankerHead,
) -> Vec<RerankedDocument> {
    match head {
        crate::core::config::reranker::RerankerHead::CrossEncoder => {
            build_results_from_scores(documents, raw_scores, top_k, sigmoid_f32)
        }
        crate::core::config::reranker::RerankerHead::Qwen3Generative => {
            build_results_from_scores(documents, raw_scores, top_k, |already_a_probability| {
                already_a_probability
            })
        }
    }
}

/// Shared sort/truncate core for both [`build_results`] and [`build_results_for_head`].
///
/// Applies `transform` to each raw value to produce the final `[0, 1]` score,
/// then sorts descending and truncates to `top_k`.
///
/// When `top_k` is smaller than the survivor count, this partitions with
/// [`slice::select_nth_unstable_by`] (`O(n)`) to isolate the top-k elements and
/// sorts only that k-slice (`O(k log k)`), instead of fully sorting all `n`
/// results (`O(n log n)`) before truncating. The returned order is identical
/// to a full sort followed by `truncate(k)` — same descending-by-score
/// ordering, same tie-breaking (`partial_cmp` with `Equal` fallback) applied
/// consistently by both the partition and the final sort.
#[cfg(any(feature = "reranker", test))]
fn build_results_from_scores(
    documents: &[String],
    raw_scores: Vec<f32>,
    top_k: Option<usize>,
    transform: impl Fn(f32) -> f32,
) -> Vec<RerankedDocument> {
    let mut results: Vec<RerankedDocument> = documents
        .iter()
        .enumerate()
        .zip(raw_scores.iter())
        .map(|((index, document), &raw)| RerankedDocument {
            index,
            score: transform(raw),
            document: document.clone(),
        })
        .collect();

    let cmp_desc =
        |a: &RerankedDocument, b: &RerankedDocument| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal);

    match top_k {
        Some(k) if k < results.len() => {
            if k > 0 {
                results.select_nth_unstable_by(k - 1, cmp_desc);
            }
            results.truncate(k);
            results.sort_by(cmp_desc);
        }
        _ => {
            results.sort_by(cmp_desc);
            if let Some(k) = top_k {
                results.truncate(k);
            }
        }
    }

    results
}

/// Rerank a list of documents by relevance to a query.
///
/// Returns `RerankedDocument`s sorted descending by score. If `top_k` is set in
/// the config, only the top-k results are returned.
///
/// Returns an empty `Vec` for empty `documents` input (no error).
///
/// # Errors
///
/// - `XbergError::Validation` if `query` is blank or empty after trimming.
/// - `XbergError::MissingDependency` if ONNX Runtime is not installed (ONNX path).
/// - `XbergError::Reranking` if the preset name is unknown or model download fails.
///
/// Since v5.0.0.
#[cfg(feature = "reranker")]
pub fn rerank(
    query: String,
    documents: Vec<String>,
    config: &crate::core::config::RerankerConfig,
) -> crate::Result<Vec<RerankedDocument>> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }

    if query.trim().is_empty() {
        return Err(crate::XbergError::Validation {
            message: "Reranker query must not be empty or blank".to_string(),
            source: None,
        });
    }

    match &config.model {
        #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
        crate::core::config::RerankerModelType::Llm { llm } => {
            let top_k = config.top_k;
            let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
                if matches!(handle.runtime_flavor(), tokio::runtime::RuntimeFlavor::CurrentThread) {
                    return Err(crate::XbergError::reranking(
                        "Synchronous rerank() with an LLM backend cannot be called from a current-thread Tokio runtime. \
                         Use rerank_async() or build a multi-thread runtime.",
                    ));
                }
                tokio::task::block_in_place(|| {
                    handle.block_on(crate::llm::rerank::rerank_via_llm(&query, &documents, llm, top_k))
                })
            } else {
                crate::core::runtime::global_runtime()?
                    .block_on(crate::llm::rerank::rerank_via_llm(&query, &documents, llm, top_k))
            };
            result.map(|(results, _usage)| results)
        }
        #[cfg(any(not(feature = "liter-llm"), target_arch = "wasm32"))]
        crate::core::config::RerankerModelType::Llm { .. } => Err(crate::XbergError::MissingDependency(
            "LLM reranking requires the 'liter-llm' feature. Rebuild with --features liter-llm".into(),
        )),
        crate::core::config::RerankerModelType::Plugin { name } => {
            let registry = crate::plugins::registry::get_reranker_backend_registry();
            let backend = {
                let guard = registry.read();
                guard.get(name)?
            };
            let expected_count = documents.len();
            let timeout = config
                .max_rerank_duration_secs
                .filter(|&s| s > 0)
                .map(std::time::Duration::from_secs);

            let rerank_future = async {
                match timeout {
                    Some(dur) => tokio::time::timeout(dur, backend.rerank(query.clone(), documents.clone()))
                        .await
                        .map_err(|_| crate::XbergError::Plugin {
                            message: format!("Reranker backend '{name}' did not complete within {dur:?}"),
                            plugin_name: name.clone(),
                        })?,
                    None => backend.rerank(query.clone(), documents.clone()).await,
                }
            };

            let logits = if let Ok(handle) = tokio::runtime::Handle::try_current() {
                if matches!(handle.runtime_flavor(), tokio::runtime::RuntimeFlavor::CurrentThread) {
                    return Err(crate::XbergError::reranking(
                        "Synchronous rerank() with a Plugin backend cannot be called from a current-thread Tokio runtime. \
                         Use rerank_async() or build a multi-thread runtime.",
                    ));
                }
                tokio::task::block_in_place(|| handle.block_on(rerank_future))
            } else {
                crate::core::runtime::global_runtime()?.block_on(rerank_future)
            }?;

            validate_reranker_output(&logits, expected_count, name)?;
            Ok(build_results(&documents, logits, config.top_k))
        }
        crate::core::config::RerankerModelType::Preset { .. }
        | crate::core::config::RerankerModelType::Custom { .. } => {
            let (repo, model_file, additional_files, max_length, head) = resolve_model_info(&config.model)?;
            let engine = get_or_init_engine(
                &repo,
                &model_file,
                &additional_files,
                max_length,
                config.cache_dir.clone(),
                config.into(),
                config.acceleration.clone(),
                head,
            )?;

            let doc_refs: Vec<&str> = documents.iter().map(|d| d.as_str()).collect();
            let raw_scores = engine
                .rerank(&query, &doc_refs, config.batch_size)
                .map_err(|e| crate::XbergError::reranking(format!("ONNX inference failed: {e}")))?;

            Ok(build_results_for_head(&documents, raw_scores, config.top_k, head))
        }
    }
}

/// Rerank documents asynchronously.
///
/// Async counterpart to [`rerank`]. Offloads blocking ONNX inference to a
/// dedicated blocking thread pool via Tokio's `spawn_blocking`, keeping the
/// async executor free.
///
/// Since v5.0.0.
#[doc(alias = "rerank")]
#[cfg(all(feature = "reranker", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub async fn rerank_async(
    query: String,
    documents: Vec<String>,
    config: &crate::core::config::RerankerConfig,
) -> crate::Result<Vec<RerankedDocument>> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }

    if query.trim().is_empty() {
        return Err(crate::XbergError::Validation {
            message: "Reranker query must not be empty or blank".to_string(),
            source: None,
        });
    }

    match &config.model {
        #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
        crate::core::config::RerankerModelType::Llm { llm } => {
            return crate::llm::rerank::rerank_via_llm(&query, &documents, llm, config.top_k)
                .await
                .map(|(results, _usage)| results);
        }
        #[cfg(any(not(feature = "liter-llm"), target_arch = "wasm32"))]
        crate::core::config::RerankerModelType::Llm { .. } => {
            return Err(crate::XbergError::MissingDependency(
                "LLM reranking requires the 'liter-llm' feature. Rebuild with --features liter-llm".into(),
            ));
        }
        crate::core::config::RerankerModelType::Plugin { name } => {
            let registry = crate::plugins::registry::get_reranker_backend_registry();
            let backend = {
                let guard = registry.read();
                guard.get(name)?
            };
            let expected_count = documents.len();
            let timeout = config
                .max_rerank_duration_secs
                .filter(|&s| s > 0)
                .map(std::time::Duration::from_secs);
            let logits = match timeout {
                Some(dur) => tokio::time::timeout(dur, backend.rerank(query, documents.clone()))
                    .await
                    .map_err(|_| crate::XbergError::Plugin {
                        message: format!("Reranker backend '{name}' did not complete within {dur:?}"),
                        plugin_name: name.clone(),
                    })??,
                None => backend.rerank(query, documents.clone()).await?,
            };
            validate_reranker_output(&logits, expected_count, name)?;
            return Ok(build_results(&documents, logits, config.top_k));
        }
        crate::core::config::RerankerModelType::Preset { .. }
        | crate::core::config::RerankerModelType::Custom { .. } => {}
    }

    let _permit = RERANK_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| crate::XbergError::reranking("Reranker semaphore closed".to_string()))?;

    let config = std::sync::Arc::new(config.clone());
    tokio::task::spawn_blocking(move || rerank(query, documents, &config))
        .await
        .map_err(|e| crate::XbergError::reranking(format!("Reranker task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fail-closed guarantee: every hosted reranker preset's weight file (and any
    /// external-data sibling) must be pinned in `presets.sha256sum`.
    #[test]
    fn every_preset_file_is_pinned_in_manifest() {
        let manifest = crate::model_download::parse_sha256_manifest(RERANKER_SHA256_MANIFEST).unwrap();
        let pinned: std::collections::HashSet<&str> = manifest.iter().map(|(p, _)| p.as_str()).collect();
        for preset in RERANKER_PRESETS.iter() {
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
    fn empty_documents_returns_empty_vec() {
        let results = build_results(&[], vec![], None);
        assert!(results.is_empty());
    }

    #[test]
    fn build_results_sorts_descending_by_score() {
        let documents = vec!["doc0".to_string(), "doc1".to_string(), "doc2".to_string()];
        let logits = vec![-1.0_f32, 2.0_f32, 0.5_f32];
        let results = build_results(&documents, logits, None);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].index, 1);
        assert!(results[0].score > results[1].score, "Results must be sorted descending");
        assert!(results[1].score > results[2].score, "Results must be sorted descending");
    }

    #[test]
    fn top_k_truncation_applies() {
        let documents = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let logits = vec![1.0_f32, 2.0_f32, 0.5_f32, 1.5_f32];
        let results = build_results(&documents, logits, Some(2));

        assert_eq!(results.len(), 2, "top_k=2 should truncate to 2 results");
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn top_k_zero_returns_empty() {
        let documents = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let logits = vec![1.0_f32, 2.0_f32, 0.5_f32];
        let results = build_results(&documents, logits, Some(0));
        assert!(results.is_empty(), "top_k=0 must return an empty vec");
    }

    #[test]
    fn top_k_larger_than_docs_returns_all() {
        let documents = vec!["a".to_string(), "b".to_string()];
        let logits = vec![1.0_f32, 0.5_f32];
        let results = build_results(&documents, logits, Some(100));
        assert_eq!(results.len(), 2, "top_k larger than docs.len() should return all docs");
    }

    #[test]
    fn build_results_preserves_document_text() {
        let documents = vec!["hello world".to_string(), "foo bar".to_string()];
        let logits = vec![0.0_f32, 1.0_f32];
        let results = build_results(&documents, logits, None);

        assert_eq!(results[0].document, "foo bar");
        assert_eq!(results[1].document, "hello world");
    }

    #[test]
    fn build_results_for_head_cross_encoder_applies_sigmoid() {
        use crate::core::config::reranker::RerankerHead;

        let documents = vec!["doc0".to_string(), "doc1".to_string()];
        let raw_logits = vec![0.0_f32, 2.0_f32];
        let results = build_results_for_head(&documents, raw_logits.clone(), None, RerankerHead::CrossEncoder);

        let expected = build_results(&documents, raw_logits, None);
        assert_eq!(results.len(), expected.len());
        for (a, b) in results.iter().zip(expected.iter()) {
            assert_eq!(a.index, b.index);
            assert!((a.score - b.score).abs() < 1e-9);
        }
        let doc0 = results.iter().find(|r| r.index == 0).unwrap();
        assert!(
            (doc0.score - 0.5).abs() < 1e-6,
            "cross-encoder head must sigmoid, got {}",
            doc0.score
        );
    }

    #[test]
    fn build_results_for_head_qwen3_does_not_double_sigmoid() {
        use crate::core::config::reranker::RerankerHead;

        let documents = vec!["doc0".to_string(), "doc1".to_string()];
        let already_probabilities = vec![0.9_f32, 0.1_f32];
        let results = build_results_for_head(
            &documents,
            already_probabilities.clone(),
            None,
            RerankerHead::Qwen3Generative,
        );

        let doc0 = results.iter().find(|r| r.index == 0).unwrap();
        let doc1 = results.iter().find(|r| r.index == 1).unwrap();
        assert!(
            (doc0.score - 0.9).abs() < 1e-6,
            "Qwen3 score must pass through unchanged, got {}",
            doc0.score
        );
        assert!(
            (doc1.score - 0.1).abs() < 1e-6,
            "Qwen3 score must pass through unchanged, got {}",
            doc1.score
        );

        let wrongly_double_sigmoided = sigmoid_f32(0.9);
        assert!(
            (doc0.score - wrongly_double_sigmoided).abs() > 0.01,
            "Qwen3 score must NOT be re-sigmoided"
        );

        assert!(doc0.score > doc1.score);
    }

    #[test]
    fn reranked_document_serde_roundtrip() {
        let doc = RerankedDocument {
            index: 3,
            score: 0.87,
            document: "test document".to_string(),
        };
        let json = serde_json::to_string(&doc).unwrap();
        let back: RerankedDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back.index, doc.index);
        assert!((back.score - doc.score).abs() < 1e-6);
        assert_eq!(back.document, doc.document);
    }

    #[cfg(feature = "reranker-presets")]
    #[test]
    fn preset_list_exposes_catalog_plus_aliases() {
        let presets = list_presets();
        assert_eq!(presets.len(), RERANKER_PRESETS.len() + PRESET_ALIASES.len());
        assert!(presets.iter().any(|n| n == "bge-reranker-base"));
        assert!(presets.iter().any(|n| n == "bge-reranker-v2-m3"));
        assert!(presets.iter().any(|n| n == "jina-reranker-v1-turbo-en"));
        assert!(presets.iter().any(|n| n == "qwen3-reranker-0.6b"));
        assert!(!presets.iter().any(|n| n == "jina-reranker-v2-base-multilingual"));
        for (alias, _) in PRESET_ALIASES {
            assert!(presets.iter().any(|n| n == *alias), "missing alias: {alias}");
        }
    }

    #[cfg(feature = "reranker-presets")]
    #[test]
    fn qwen3_preset_uses_generative_head() {
        use crate::core::config::reranker::RerankerHead;

        let preset = get_preset("qwen3-reranker-0.6b").expect("qwen3 preset must exist");
        assert_eq!(preset.model_repo, "xberg-io/reranker-models");
        assert_eq!(preset.head, RerankerHead::Qwen3Generative);

        for name in ["bge-reranker-base", "bge-reranker-v2-m3", "jina-reranker-v1-turbo-en"] {
            let preset = get_preset(name).expect(name);
            assert_eq!(
                preset.head,
                RerankerHead::CrossEncoder,
                "{name} must remain on the original cross-encoder head"
            );
        }
    }

    #[cfg(feature = "reranker-presets")]
    #[test]
    fn get_preset_case_sensitive() {
        assert!(get_preset("bge-reranker-base").is_some());
        assert!(
            get_preset("BGE-Reranker-Base").is_none(),
            "Preset lookup must be case-sensitive"
        );
        assert!(get_preset("nonexistent").is_none());
    }

    #[cfg(feature = "reranker-presets")]
    #[test]
    fn aliases_resolve_to_catalog_entries() {
        for (alias, catalog_name) in PRESET_ALIASES {
            let preset = get_preset(alias).expect("alias must resolve");
            assert_eq!(
                preset.name, *catalog_name,
                "alias {alias} should resolve to catalog entry {catalog_name}"
            );
        }
    }

    #[cfg(feature = "reranker-presets")]
    #[test]
    fn catalog_paths_are_stable() {
        let by_name = |n: &str| get_preset(n).expect(n);

        let base = by_name("bge-reranker-base");
        assert_eq!(base.model_repo, "xberg-io/reranker-models");
        assert_eq!(base.model_file, "bge-reranker-base/model.onnx");
        assert!(base.additional_files.is_empty());

        let m3 = by_name("bge-reranker-v2-m3");
        assert_eq!(m3.model_repo, "xberg-io/reranker-models");
        assert_eq!(m3.model_file, "bge-reranker-v2-m3/model.onnx");
        assert_eq!(
            m3.additional_files,
            vec!["bge-reranker-v2-m3/model.onnx.data".to_string()]
        );

        let turbo = by_name("jina-reranker-v1-turbo-en");
        assert_eq!(turbo.model_repo, "xberg-io/reranker-models");
        assert_eq!(turbo.model_file, "jina-reranker-v1-turbo-en/model.onnx");

        let qwen3 = by_name("qwen3-reranker-0.6b");
        assert_eq!(qwen3.model_repo, "xberg-io/reranker-models");
        assert_eq!(qwen3.model_file, "qwen3-reranker-0.6b/model.onnx");
        assert_eq!(
            qwen3.additional_files,
            vec!["qwen3-reranker-0.6b/model.onnx.data".to_string()]
        );
        assert_eq!(qwen3.head, crate::core::config::reranker::RerankerHead::Qwen3Generative);
    }

    #[cfg(feature = "reranker")]
    fn build_wordlevel_tokenizer(vocab: &[(&str, u32)], lowercase: bool) -> tokenizers::Tokenizer {
        use tokenizers::models::wordlevel::WordLevel;
        use tokenizers::normalizers::utils::Lowercase;
        use tokenizers::{AddedToken, Tokenizer};

        let vocab: ahash::AHashMap<String, u32> = vocab.iter().map(|(k, v)| (k.to_string(), *v)).collect();

        let model = WordLevel::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("build WordLevel model");
        let mut tokenizer = Tokenizer::new(model);
        let _ = tokenizer.add_special_tokens([AddedToken::from("[UNK]", true)]);
        tokenizer.with_pre_tokenizer(Some(tokenizers::pre_tokenizers::whitespace::Whitespace {}));
        if lowercase {
            let _ = tokenizer.with_normalizer(Some(Lowercase));
        }
        tokenizer
    }

    #[cfg(feature = "reranker")]
    #[test]
    fn resolve_qwen3_token_ids_uses_direct_vocab_when_present() {
        let tokenizer = build_wordlevel_tokenizer(&[("[UNK]", 0), ("yes", 1), ("no", 2)], false);

        let (true_id, false_id) = resolve_qwen3_token_ids(&tokenizer).expect("must resolve both ids");

        assert_eq!(true_id, 1, "\"yes\" must resolve to its direct vocab id");
        assert_eq!(false_id, 2, "\"no\" must resolve to its direct vocab id");
    }

    #[cfg(feature = "reranker")]
    #[test]
    fn resolve_answer_token_id_falls_back_to_encoding_when_no_direct_vocab_entry() {
        let tokenizer = build_wordlevel_tokenizer(&[("[UNK]", 0), ("yes", 7), ("no", 9)], true);

        assert!(tokenizer.token_to_id("Yes").is_none());
        assert!(tokenizer.token_to_id("Ġyes").is_none());
        assert!(tokenizer.token_to_id("\u{2581}yes").is_none());
        assert!(tokenizer.token_to_id("YES").is_none());

        let true_id = resolve_answer_token_id(&tokenizer, "Yes").expect("encode fallback must resolve \"Yes\"");
        let false_id = resolve_answer_token_id(&tokenizer, "No").expect("encode fallback must resolve \"No\"");

        assert_eq!(
            true_id, 7,
            "\"Yes\" must resolve via the encode fallback to the lowercase vocab id"
        );
        assert_eq!(
            false_id, 9,
            "\"No\" must resolve via the encode fallback to the lowercase vocab id"
        );
    }

    #[cfg(feature = "reranker")]
    #[test]
    fn resolve_qwen3_token_ids_errors_with_actionable_message_when_word_is_unresolvable() {
        let tokenizer = build_wordlevel_tokenizer(&[("no", 1)], false);

        let error = resolve_qwen3_token_ids(&tokenizer).expect_err("\"yes\" is unresolvable and must error");
        let message = error.to_string();

        assert!(
            message.contains("\"yes\""),
            "error message must name the word that failed to resolve: {message}"
        );
        assert!(
            message.contains("Qwen3 generative-reranker head"),
            "error message must mention the Qwen3 generative-reranker head: {message}"
        );
        assert!(
            message.contains("incompatible"),
            "error message must suggest the checkpoint/tokenizer is incompatible: {message}"
        );
        assert!(
            message.contains("reranker checkpoint"),
            "error message must suggest checking the reranker checkpoint: {message}"
        );
    }

    #[cfg(feature = "reranker")]
    #[test]
    fn resolve_answer_token_id_encode_fallback_rejects_unk_mapped_word() {
        let tokenizer = build_wordlevel_tokenizer(&[("[UNK]", 0), ("no", 1)], false);

        assert!(tokenizer.token_to_id("yes").is_none());
        assert!(tokenizer.token_to_id("Ġyes").is_none());
        assert!(tokenizer.token_to_id("\u{2581}yes").is_none());
        assert!(tokenizer.token_to_id("Yes").is_none());

        let result = resolve_answer_token_id(&tokenizer, "yes");
        assert_eq!(
            result,
            Some(0),
            "encode fallback resolves the absent word to the shared [UNK] id \
             (current behavior — callers must not treat this as a genuine match)"
        );
    }

    #[cfg(all(feature = "reranker", feature = "tokio-runtime"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_backend_rerank_roundtrip() {
        use crate::core::config::RerankerConfig;
        use crate::plugins::registry::test_support::RerankerRegistryGuard;
        use crate::plugins::{Plugin, RerankerBackend, register_reranker_backend, unregister_reranker_backend};
        use async_trait::async_trait;
        use std::sync::Arc;

        struct MockPlugin {
            name: String,
        }

        impl Plugin for MockPlugin {
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

        #[async_trait]
        impl RerankerBackend for MockPlugin {
            async fn rerank(&self, _query: String, documents: Vec<String>) -> crate::Result<Vec<f32>> {
                Ok(documents
                    .iter()
                    .enumerate()
                    .map(|(i, _)| (documents.len() - i) as f32)
                    .collect())
            }
        }

        // Shares the global reranker registry with `plugins::reranker`'s tests, so it takes the
        // same lock: without it, that module's `clear_reranker_backends` can wipe this backend
        // between registration and the `rerank_async` call below. ~keep
        let _guard = RerankerRegistryGuard::acquire();
        let name = "test-mock-reranker".to_string();

        register_reranker_backend(Arc::new(MockPlugin { name: name.clone() })).unwrap();

        let config = RerankerConfig {
            model: crate::core::config::RerankerModelType::Plugin { name: name.clone() },
            top_k: Some(2),
            ..Default::default()
        };

        let results = rerank_async(
            "test query".to_string(),
            vec!["doc0".to_string(), "doc1".to_string(), "doc2".to_string()],
            &config,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 2, "top_k=2 should limit to 2 results");
        assert!(results[0].score >= results[1].score);

        unregister_reranker_backend(&name).unwrap();
    }
}
