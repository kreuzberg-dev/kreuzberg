//! `xberg-gliner` ONNX backend for named-entity recognition.
//!
//! Runs span-mode [GLiNER](https://huggingface.co/gliner-community) models
//! exported to ONNX and published under `xberg-io/gliner-models`. The source
//! model lineage is `gliner-community`; xberg consumes only the checked ONNX
//! runtime artifacts and tokenizer files.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use ahash::AHashMap;
use async_trait::async_trait;
use parking_lot::RwLock;
use xberg_gliner::{Gliner, Parameters, RuntimeConfig, TextInput};

use crate::Result;
use crate::types::entity::{Entity, EntityCategory};

use super::backend::NerBackend;
use super::offsets;

/// Hugging Face repository that stores xberg-managed GLiNER ONNX exports.
pub const GLINER_MODELS_REPO: &str = "xberg-io/gliner-models";

/// Immutable Hugging Face revision containing the verified GLiNER artifacts.
pub const GLINER_MODELS_REVISION: &str = "afb0faaa3c8e7d0de7796bd37e625026ff635fe0";

/// SHA-256 manifest pinning every hosted GLiNER model/tokenizer file, checked in as the
/// single source of truth. Trust attaches to the manifest, not the host — a changed or
/// tampered upstream file fails verification instead of feeding wrong weights into
/// inference.
pub(crate) const GLINER_SHA256_MANIFEST: &str = include_str!("gliner-models.sha256");

/// Default xberg GLiNER model alias.
pub const DEFAULT_MODEL_NAME: &str = "balanced";

/// Backwards-compatible constant for code that described the old default as a repository.
pub const DEFAULT_MODEL_REPO: &str = GLINER_MODELS_REPO;

/// Canonical GLiNER model ids xberg knows how to download.
///
/// Used by `xberg cache warm --all-ner-models` to pre-fetch the supported fleet.
pub const KNOWN_MODELS: &[&str] = &["gliner_small-v2.5", "gliner_medium-v2.5", "gliner_large-v2.5"];

/// Default entity labels used when the caller supplies an empty `categories` slice.
const DEFAULT_LABELS: &[&str] = &["person", "organization", "location", "date", "email"];

/// Number of windows submitted to one GLiNER inference call.
///
/// Must stay at or below `xberg_gliner`'s own batch cap (32). Kept well under it
/// so a long document's windows are processed in bounded-memory groups instead
/// of one enormous padded batch.
const WINDOW_BATCH_SIZE: usize = 8;

type BackendCache = AHashMap<GlineBackendCacheKey, Arc<GlineBackend>>;

static BACKEND_CACHE: LazyLock<RwLock<BackendCache>> = LazyLock::new(|| RwLock::new(AHashMap::default()));

#[derive(Debug, Clone, Copy)]
struct GlinerModelDefinition {
    id: &'static str,
    aliases: &'static [&'static str],
    upstream_repo: &'static str,
    model_file: &'static str,
    model_size_bytes: u64,
    tokenizer_file: &'static str,
    tokenizer_size_bytes: u64,
}

// Sizes reported by Hugging Face's expanded recursive tree metadata for
// `xberg-io/gliner-models` at `GLINER_MODELS_REVISION`. ~keep
const GLINER_MODELS: &[GlinerModelDefinition] = &[
    GlinerModelDefinition {
        id: "gliner_small-v2.5",
        aliases: &["fast"],
        upstream_repo: "gliner-community/gliner_small-v2.5",
        model_file: "models/gliner_small-v2.5/span/fp32/model.onnx",
        model_size_bytes: 664_780_382,
        tokenizer_file: "models/gliner_small-v2.5/span/fp32/tokenizer.json",
        tokenizer_size_bytes: 8_649_232,
    },
    GlinerModelDefinition {
        id: "gliner_medium-v2.5",
        aliases: &["balanced", "multilingual"],
        upstream_repo: "gliner-community/gliner_medium-v2.5",
        model_file: "models/gliner_medium-v2.5/span/fp32/model.onnx",
        model_size_bytes: 835_514_666,
        tokenizer_file: "models/gliner_medium-v2.5/span/fp32/tokenizer.json",
        tokenizer_size_bytes: 8_649_232,
    },
    GlinerModelDefinition {
        id: "gliner_large-v2.5",
        aliases: &["quality"],
        upstream_repo: "gliner-community/gliner_large-v2.5",
        model_file: "models/gliner_large-v2.5/span/fp32/model.onnx",
        model_size_bytes: 1_840_548_694,
        tokenizer_file: "models/gliner_large-v2.5/span/fp32/tokenizer.json",
        tokenizer_size_bytes: 8_649_232,
    },
];

#[derive(Debug, Clone)]
struct GlinerModelFiles {
    definition: GlinerModelDefinition,
    model_path: PathBuf,
    tokenizer_path: PathBuf,
}

#[cfg_attr(alef, alef(skip))]
/// A single GLiNER model artifact entry in the cache manifest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GlinerManifestEntry {
    /// Relative path within the Hugging Face Hub cache directory.
    pub relative_path: String,
    /// Expected SHA-256 checksum from the pinned Hugging Face revision.
    pub sha256: String,
    /// Expected file size in bytes from the pinned Hugging Face revision.
    pub size_bytes: u64,
    /// Hugging Face source URL for downloading.
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GlineBackendCacheKey {
    model_id: String,
    thread_budget: usize,
}

/// Eagerly fetch a GLiNER model (ONNX + tokenizer) into the Hugging Face cache.
///
/// `name` must be a supported xberg GLiNER model alias or catalog id. Runtime
/// artifacts are downloaded from `xberg-io/gliner-models` at an immutable
/// revision. `cache_dir`, when provided, overrides the Hugging Face cache root.
pub fn download_model(name: &str, cache_dir: Option<PathBuf>) -> Result<PathBuf> {
    Ok(ensure_model(name, cache_dir)?.model_path)
}

fn ensure_model(name: &str, cache_dir: Option<PathBuf>) -> Result<GlinerModelFiles> {
    let definition = resolve_model(name)?;
    let checksums = load_gliner_checksums()?;
    let model_sha256 = required_checksum(&checksums, definition.model_file)?;
    let tokenizer_sha256 = required_checksum(&checksums, definition.tokenizer_file)?;
    let model_path = crate::model_download::hf_resolve_file(
        GLINER_MODELS_REPO,
        definition.model_file,
        Some(GLINER_MODELS_REVISION),
        cache_dir.as_deref(),
        Some(model_sha256),
    )
    .map_err(|error| gliner_download_error(definition, "model", error))?;
    let tokenizer_path = crate::model_download::hf_resolve_file(
        GLINER_MODELS_REPO,
        definition.tokenizer_file,
        Some(GLINER_MODELS_REVISION),
        cache_dir.as_deref(),
        Some(tokenizer_sha256),
    )
    .map_err(|error| gliner_download_error(definition, "tokenizer", error))?;

    tracing::info!(
        model = definition.id,
        upstream = definition.upstream_repo,
        model_path = %model_path.display(),
        tokenizer_path = %tokenizer_path.display(),
        "xberg-gliner model downloaded"
    );

    Ok(GlinerModelFiles {
        definition,
        model_path,
        tokenizer_path,
    })
}

fn gliner_download_error(definition: GlinerModelDefinition, artifact: &str, error: String) -> crate::XbergError {
    crate::XbergError::Plugin {
        message: format!(
            "Failed to resolve GLiNER {artifact} '{}' from {}@{}: {error}",
            definition.id, GLINER_MODELS_REPO, GLINER_MODELS_REVISION
        ),
        plugin_name: "ner-gliner".to_string(),
    }
}

/// Returns the GLiNER files expected by `xberg cache manifest`.
#[cfg_attr(alef, alef(skip))]
pub fn manifest() -> Vec<GlinerManifestEntry> {
    let checksums = load_gliner_checksums().expect("vendored GLiNER checksum manifest must be valid");
    let mut entries = Vec::new();

    for definition in GLINER_MODELS {
        let cache_prefix = format!("models--xberg-io--gliner-models/snapshots/{GLINER_MODELS_REVISION}");
        entries.push(GlinerManifestEntry {
            relative_path: format!("{cache_prefix}/{}", definition.model_file),
            sha256: required_checksum(&checksums, definition.model_file)
                .expect("declared GLiNER model must have a checksum")
                .to_string(),
            size_bytes: definition.model_size_bytes,
            source_url: format!(
                "https://huggingface.co/{GLINER_MODELS_REPO}/resolve/{GLINER_MODELS_REVISION}/{}",
                definition.model_file
            ),
        });
        entries.push(GlinerManifestEntry {
            relative_path: format!("{cache_prefix}/{}", definition.tokenizer_file),
            sha256: required_checksum(&checksums, definition.tokenizer_file)
                .expect("declared GLiNER tokenizer must have a checksum")
                .to_string(),
            size_bytes: definition.tokenizer_size_bytes,
            source_url: format!(
                "https://huggingface.co/{GLINER_MODELS_REPO}/resolve/{GLINER_MODELS_REVISION}/{}",
                definition.tokenizer_file
            ),
        });
    }
    entries
}

fn load_gliner_checksums() -> Result<HashMap<String, String>> {
    parse_checksums(GLINER_SHA256_MANIFEST)
}

fn parse_checksums(content: &str) -> Result<HashMap<String, String>> {
    let entries = crate::model_download::parse_sha256_manifest(content)
        .map_err(|e| crate::XbergError::validation(format!("Invalid GLiNER checksums file: {e}")))?;
    Ok(entries.into_iter().collect())
}

fn required_checksum<'a>(checksums: &'a HashMap<String, String>, path: &str) -> Result<&'a str> {
    checksums.get(path).map(String::as_str).ok_or_else(|| {
        crate::XbergError::validation(format!(
            "GLiNER checksums file does not include required artifact '{path}'"
        ))
    })
}

fn resolve_model(name: &str) -> Result<GlinerModelDefinition> {
    let requested = name.trim();
    if requested.is_empty() {
        return Err(crate::XbergError::validation("GLiNER model name must not be empty"));
    }

    GLINER_MODELS
        .iter()
        .copied()
        .find(|definition| definition.id == requested || definition.aliases.contains(&requested))
        .ok_or_else(|| {
            crate::XbergError::validation(format!(
                "Unknown GLiNER model '{requested}'. Available models: {}",
                available_model_names().join(", ")
            ))
        })
}

fn requested_model_name(model_name: Option<&str>) -> Result<String> {
    let requested = model_name.unwrap_or(DEFAULT_MODEL_NAME).trim();
    if requested.is_empty() {
        return Err(crate::XbergError::validation("GLiNER model name must not be empty"));
    }
    Ok(requested.to_string())
}

fn backend_cache_key(model_name: Option<&str>, thread_budget: usize) -> Result<GlineBackendCacheKey> {
    let requested = requested_model_name(model_name)?;
    let definition = resolve_model(&requested)?;
    Ok(GlineBackendCacheKey {
        model_id: definition.id.to_string(),
        thread_budget,
    })
}

fn available_model_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    for definition in GLINER_MODELS {
        names.push(definition.id);
        names.extend(definition.aliases);
    }
    names.sort_unstable();
    names
}

/// Map an [`EntityCategory`] to the label string GLiNER expects.
fn category_to_label(category: &EntityCategory) -> String {
    match category {
        EntityCategory::Person => "person".to_string(),
        EntityCategory::Organization => "organization".to_string(),
        EntityCategory::Location => "location".to_string(),
        EntityCategory::Date => "date".to_string(),
        EntityCategory::Time => "time".to_string(),
        EntityCategory::Money => "money".to_string(),
        EntityCategory::Percent => "percent".to_string(),
        EntityCategory::Email => "email".to_string(),
        EntityCategory::Phone => "phone".to_string(),
        EntityCategory::Url => "url".to_string(),
        EntityCategory::Custom(label) => label.clone(),
    }
}

/// Reverse-map a GLiNER class string to an [`EntityCategory`].
fn label_to_category(class: &str) -> EntityCategory {
    match class {
        "person" => EntityCategory::Person,
        "organization" => EntityCategory::Organization,
        "location" => EntityCategory::Location,
        "date" => EntityCategory::Date,
        "time" => EntityCategory::Time,
        "money" => EntityCategory::Money,
        "percent" => EntityCategory::Percent,
        "email" => EntityCategory::Email,
        "phone" => EntityCategory::Phone,
        "url" => EntityCategory::Url,
        other => EntityCategory::Custom(other.to_string()),
    }
}

fn default_labels() -> Vec<String> {
    DEFAULT_LABELS.iter().map(|label| (*label).to_string()).collect()
}

/// Narrow a GLiNER byte offset to the `u32` the entity type carries.
///
/// `text` is only used for its length: the matched span is the PII this
/// pipeline exists to hide, so it must never reach an error message, a log
/// line, or telemetry.
fn checked_offset_to_u32(offset: usize, field: &str, text: &str, class: &str) -> Result<u32> {
    u32::try_from(offset).map_err(|error| {
        let span_len = text.len();
        crate::XbergError::validation_with_source(
            format!(
                "GLiNER returned {field} offset {offset} for class '{class}' in a {span_len}-byte span, \
                 which exceeds the u32 entity offset limit"
            ),
            error,
        )
    })
}

fn prepare_labels(categories: &[EntityCategory], custom_labels: &[String]) -> Vec<String> {
    if categories.is_empty() && custom_labels.is_empty() {
        return default_labels();
    }

    let mut seen = HashSet::new();
    let mut labels = Vec::new();
    for label in categories
        .iter()
        .map(category_to_label)
        .chain(custom_labels.iter().cloned())
    {
        let label = label.trim();
        if !label.is_empty() && seen.insert(label.to_string()) {
            labels.push(label.to_string());
        }
    }

    if labels.is_empty() { default_labels() } else { labels }
}

fn get_or_insert_arc<K, V, F>(cache: &RwLock<AHashMap<K, Arc<V>>>, key: K, build: F) -> Result<Arc<V>>
where
    K: Clone + Eq + Hash,
    F: FnOnce() -> Result<V>,
{
    {
        let cache = cache.read();
        if let Some(value) = cache.get(&key) {
            return Ok(Arc::clone(value));
        }
    }

    let mut cache = cache.write();
    if let Some(value) = cache.get(&key) {
        return Ok(Arc::clone(value));
    }

    let value = Arc::new(build()?);
    cache.insert(key, Arc::clone(&value));
    Ok(value)
}

/// `xberg-gliner` ONNX backend wrapper.
///
/// Holds an initialised GLiNER span-mode model. Inference is synchronous and
/// internally serialized around the underlying ONNX Runtime session.
pub struct GlineBackend {
    /// xberg GLiNER model alias or catalog id used to load this model.
    pub repo_id: String,
    /// Local path to the cached ONNX model file.
    pub model_path: PathBuf,
    /// Local path to the cached tokenizer file.
    pub tokenizer_path: PathBuf,
    model: Arc<Gliner>,
    /// Word-token budget of a single inference call, taken from the
    /// `Parameters` this model was built with. Input longer than this is
    /// windowed rather than truncated (xberg-io/xberg#262).
    window_tokens: usize,
}

impl GlineBackend {
    /// Build a backend for `model_name`, or the default model when `None`.
    ///
    /// Downloads the ONNX weights and tokenizer from `xberg-io/gliner-models`
    /// on first use. After this returns, inference is available without
    /// further network I/O.
    pub fn new(model_name: Option<&str>) -> Result<Self> {
        let thread_budget = crate::core::config::concurrency::resolve_thread_budget(None);
        Self::new_with_thread_budget(model_name, thread_budget)
    }

    fn new_with_thread_budget(model_name: Option<&str>, thread_budget: usize) -> Result<Self> {
        let requested = requested_model_name(model_name)?;
        let files = ensure_model(&requested, None)?;
        let parameters = Parameters::default();
        // `TokenizedInput::from` truncates the word stream at `max_length`, so this
        // is the hard ceiling a single inference call can actually see. ~keep
        let window_tokens = parameters.max_length.unwrap_or(offsets::GLINER_WINDOW_TOKENS);
        let gliner = Gliner::with_runtime(
            parameters,
            RuntimeConfig::default().with_intra_threads(thread_budget),
            &files.tokenizer_path,
            &files.model_path,
        )
        .map_err(|error| crate::XbergError::Plugin {
            message: format!("Failed to initialise GLiNER model '{}': {error}", files.definition.id),
            plugin_name: "ner-gliner".to_string(),
        })?;
        Ok(Self {
            repo_id: files.definition.id.to_string(),
            model_path: files.model_path,
            tokenizer_path: files.tokenizer_path,
            model: Arc::new(gliner),
            window_tokens,
        })
    }
}

pub(crate) fn get_or_init_backend(model_name: Option<&str>) -> Result<Arc<GlineBackend>> {
    let thread_budget = crate::core::config::concurrency::resolve_thread_budget(None);
    let key = backend_cache_key(model_name, thread_budget)?;
    let model_id = key.model_id.clone();

    get_or_insert_arc(&BACKEND_CACHE, key, || {
        GlineBackend::new_with_thread_budget(Some(&model_id), thread_budget)
    })
}

#[async_trait]
impl NerBackend for GlineBackend {
    async fn detect(&self, text: &str, categories: &[EntityCategory]) -> Result<Vec<Entity>> {
        let labels = prepare_labels(categories, &[]);
        self.detect_labels(text, labels).await
    }

    /// Native zero-shot multi-label inference: passes the union of `categories`
    /// and `custom_labels` to a single GLiNER inference call.
    async fn detect_with_custom(
        &self,
        text: &str,
        categories: &[EntityCategory],
        custom_labels: &[String],
    ) -> Result<Vec<Entity>> {
        let labels = prepare_labels(categories, custom_labels);
        self.detect_labels(text, labels).await
    }
}

impl GlineBackend {
    /// Run inference over `text`, windowing it so nothing past the model's word
    /// budget is silently dropped.
    ///
    /// The GLiNER preprocessor truncates its input at `Parameters::max_length`
    /// words. Feeding a long document in one call therefore returned entities
    /// for the head only, and the PII in the tail was never redacted
    /// (xberg-io/xberg#262). Windows overlap so a mention on a boundary is
    /// still seen whole by one of them, and the per-window spans are folded
    /// back into source coordinates by
    /// [`offsets::merge_windowed_entities`].
    async fn detect_labels(&self, text: &str, labels: Vec<String>) -> Result<Vec<Entity>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let windows = offsets::split_into_windows(text, self.window_tokens, offsets::GLINER_WINDOW_OVERLAP_TOKENS);
        if windows.is_empty() {
            return Ok(Vec::new());
        }
        let window_offsets: Vec<usize> = windows.iter().map(|window| window.byte_offset).collect();

        let backend = Arc::clone(&self.model);
        let model_path = self.model_path.clone();
        let tokenizer_path = self.tokenizer_path.clone();

        let per_window = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<Entity>>> {
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let mut per_window: Vec<Vec<Entity>> = Vec::with_capacity(windows.len());

            for batch in windows.chunks(WINDOW_BATCH_SIZE) {
                let texts: Vec<&str> = batch.iter().map(|window| window.text.as_str()).collect();
                let input = TextInput::from_str(&texts, &label_refs).map_err(|error| crate::XbergError::Plugin {
                    message: format!("Failed to build GLiNER input: {error}"),
                    plugin_name: "ner-gliner".to_string(),
                })?;
                let output = backend.inference(input).map_err(|error| crate::XbergError::Plugin {
                    message: format!(
                        "GLiNER inference failed for model '{}' (tokenizer '{}'): {error}",
                        model_path.display(),
                        tokenizer_path.display()
                    ),
                    plugin_name: "ner-gliner".to_string(),
                })?;

                // One span list per batched window; pad if the engine returns fewer so
                // window offsets stay aligned with their detections. ~keep
                let mut batch_spans = output.spans;
                batch_spans.resize_with(batch.len(), Vec::new);
                for spans in batch_spans.into_iter().take(batch.len()) {
                    per_window.push(spans_to_entities(spans)?);
                }
            }

            Ok(per_window)
        })
        .await
        .map_err(|error| crate::XbergError::Plugin {
            message: format!("GLiNER spawn_blocking task panicked: {error}"),
            plugin_name: "ner-gliner".to_string(),
        })??;

        Ok(offsets::merge_windowed_entities(&window_offsets, per_window))
    }
}

/// Convert one window's decoded spans into window-relative entities.
fn spans_to_entities(spans: Vec<xberg_gliner::Span>) -> Result<Vec<Entity>> {
    spans
        .into_iter()
        .map(|span| {
            let (start, end) = span.offsets();
            let start = checked_offset_to_u32(start, "start", span.text(), span.class())?;
            let end = checked_offset_to_u32(end, "end", span.text(), span.class())?;
            Ok(Entity {
                category: label_to_category(span.class()),
                text: span.text().to_string(),
                start,
                end,
                confidence: Some(span.probability()),
            })
        })
        .collect()
}

#[cfg(all(test, feature = "ner-onnx"))]
mod tests {
    use super::*;

    #[test]
    fn category_to_label_maps_all_canonical_variants() {
        assert_eq!(category_to_label(&EntityCategory::Person), "person");
        assert_eq!(category_to_label(&EntityCategory::Organization), "organization");
        assert_eq!(category_to_label(&EntityCategory::Location), "location");
        assert_eq!(category_to_label(&EntityCategory::Date), "date");
        assert_eq!(category_to_label(&EntityCategory::Time), "time");
        assert_eq!(category_to_label(&EntityCategory::Money), "money");
        assert_eq!(category_to_label(&EntityCategory::Percent), "percent");
        assert_eq!(category_to_label(&EntityCategory::Email), "email");
        assert_eq!(category_to_label(&EntityCategory::Phone), "phone");
        assert_eq!(category_to_label(&EntityCategory::Url), "url");
        assert_eq!(
            category_to_label(&EntityCategory::Custom("vessel".to_string())),
            "vessel"
        );
    }

    #[test]
    fn label_to_category_maps_all_canonical_labels() {
        assert_eq!(label_to_category("person"), EntityCategory::Person);
        assert_eq!(label_to_category("organization"), EntityCategory::Organization);
        assert_eq!(label_to_category("location"), EntityCategory::Location);
        assert_eq!(label_to_category("date"), EntityCategory::Date);
        assert_eq!(label_to_category("time"), EntityCategory::Time);
        assert_eq!(label_to_category("money"), EntityCategory::Money);
        assert_eq!(label_to_category("percent"), EntityCategory::Percent);
        assert_eq!(label_to_category("email"), EntityCategory::Email);
        assert_eq!(label_to_category("phone"), EntityCategory::Phone);
        assert_eq!(label_to_category("url"), EntityCategory::Url);
        assert_eq!(
            label_to_category("unknown_label"),
            EntityCategory::Custom("unknown_label".to_string())
        );
    }

    #[test]
    fn category_to_label_roundtrips() {
        let categories = [
            EntityCategory::Person,
            EntityCategory::Organization,
            EntityCategory::Location,
            EntityCategory::Date,
            EntityCategory::Time,
            EntityCategory::Money,
            EntityCategory::Percent,
            EntityCategory::Email,
            EntityCategory::Phone,
            EntityCategory::Url,
        ];
        for category in &categories {
            let label = category_to_label(category);
            assert_eq!(
                label_to_category(&label),
                *category,
                "roundtrip failed for {category:?}"
            );
        }
    }

    #[test]
    fn prepare_labels_uses_defaults_when_inputs_are_empty() {
        assert_eq!(
            prepare_labels(&[], &[]),
            vec![
                "person".to_string(),
                "organization".to_string(),
                "location".to_string(),
                "date".to_string(),
                "email".to_string(),
            ]
        );
    }

    #[test]
    fn prepare_labels_deduplicates_categories_and_custom_labels() {
        let custom_labels = vec![
            "person".to_string(),
            "treatment".to_string(),
            "organization".to_string(),
            "treatment".to_string(),
        ];
        let labels = prepare_labels(
            &[
                EntityCategory::Person,
                EntityCategory::Organization,
                EntityCategory::Person,
                EntityCategory::Custom("treatment".to_string()),
            ],
            &custom_labels,
        );

        assert_eq!(
            labels,
            vec![
                "person".to_string(),
                "organization".to_string(),
                "treatment".to_string(),
            ]
        );
    }

    #[test]
    fn prepare_labels_ignores_blank_custom_labels() {
        let labels = prepare_labels(
            &[EntityCategory::Custom("   ".to_string())],
            &["".to_string(), "  ".to_string()],
        );

        assert_eq!(labels, default_labels());
    }

    #[test]
    fn manifest_includes_gliner_models() {
        let entries = manifest();
        const EXPECTED_TOTAL_SIZE_BYTES: u64 = 3_366_791_438;

        assert_eq!(entries.len(), 6);
        assert_eq!(
            entries.iter().map(|entry| entry.size_bytes).sum::<u64>(),
            EXPECTED_TOTAL_SIZE_BYTES
        );
        assert!(entries.iter().all(|entry| entry.size_bytes > 0));
        assert!(entries.iter().any(|entry| {
            entry.relative_path
                == format!(
                    "models--xberg-io--gliner-models/snapshots/{GLINER_MODELS_REVISION}/models/gliner_medium-v2.5/span/fp32/model.onnx"
                )
                && entry.source_url.contains(GLINER_MODELS_REPO)
        }));
        assert!(entries.iter().any(|entry| {
            entry.relative_path
                == format!(
                    "models--xberg-io--gliner-models/snapshots/{GLINER_MODELS_REVISION}/models/gliner_medium-v2.5/span/fp32/tokenizer.json"
                )
                && entry.source_url.contains(GLINER_MODELS_REPO)
        }));
        assert!(entries.iter().all(|entry| {
            !entry.sha256.is_empty()
                && entry
                    .source_url
                    .contains(&format!("/resolve/{GLINER_MODELS_REVISION}/"))
        }));
    }

    #[test]
    fn manifest_uses_sizes_from_pinned_hugging_face_metadata() {
        let entries = manifest();
        let expected = [
            ("models/gliner_small-v2.5/span/fp32/model.onnx", 664_780_382),
            ("models/gliner_small-v2.5/span/fp32/tokenizer.json", 8_649_232),
            ("models/gliner_medium-v2.5/span/fp32/model.onnx", 835_514_666),
            ("models/gliner_medium-v2.5/span/fp32/tokenizer.json", 8_649_232),
            ("models/gliner_large-v2.5/span/fp32/model.onnx", 1_840_548_694),
            ("models/gliner_large-v2.5/span/fp32/tokenizer.json", 8_649_232),
        ];

        for (suffix, expected_size) in expected {
            let entry = entries
                .iter()
                .find(|entry| entry.relative_path.ends_with(suffix))
                .unwrap_or_else(|| panic!("missing manifest entry for {suffix}"));
            assert_eq!(entry.size_bytes, expected_size, "incorrect pinned size for {suffix}");
        }
    }

    #[test]
    fn load_gliner_checksums_reads_vendored_manifest_without_network() {
        let checksums = load_gliner_checksums().expect("checksums");
        assert_eq!(
            required_checksum(&checksums, "models/gliner_medium-v2.5/span/fp32/model.onnx").expect("checksum"),
            "014f8d7185ccd3e1d37af3932a7ade31bea20016359924eb25f35efb8572cc06"
        );
    }

    /// Every declared model's ONNX and tokenizer files must be pinned in the vendored
    /// manifest, and the manifest must contain exactly the fleet's six entries — no
    /// stale or missing artifacts.
    #[test]
    fn every_gliner_model_file_is_pinned_in_manifest() {
        let manifest = crate::model_download::parse_sha256_manifest(GLINER_SHA256_MANIFEST).unwrap();
        assert_eq!(manifest.len(), 6, "expected exactly 6 pinned GLiNER artifacts");
        let pinned: std::collections::HashSet<&str> = manifest.iter().map(|(p, _)| p.as_str()).collect();
        for definition in GLINER_MODELS {
            assert!(
                pinned.contains(definition.model_file),
                "GLiNER model {} model_file {} is not pinned in gliner-models.sha256",
                definition.id,
                definition.model_file
            );
            assert!(
                pinned.contains(definition.tokenizer_file),
                "GLiNER model {} tokenizer_file {} is not pinned in gliner-models.sha256",
                definition.id,
                definition.tokenizer_file
            );
        }
    }

    #[test]
    fn backend_cache_key_uses_canonical_model_and_runtime_config() {
        let default_key = backend_cache_key(None, 4).expect("default key");
        let alias_key = backend_cache_key(Some("balanced"), 4).expect("alias key");
        let id_key = backend_cache_key(Some("gliner_medium-v2.5"), 4).expect("id key");
        let different_runtime_key = backend_cache_key(Some("balanced"), 2).expect("runtime key");

        assert_eq!(default_key, alias_key);
        assert_eq!(alias_key, id_key);
        assert_ne!(alias_key, different_runtime_key);
    }

    #[test]
    fn backend_cache_key_rejects_empty_model_name_without_downloading() {
        assert!(backend_cache_key(Some("   "), 4).is_err());
    }

    #[test]
    fn known_models_are_unique_canonical_download_targets() {
        let mut names = std::collections::HashSet::new();
        let mut model_ids = std::collections::HashSet::new();

        for name in KNOWN_MODELS {
            assert!(names.insert(*name), "duplicate known model name: {name}");

            let definition = resolve_model(name).expect("known model resolves");
            assert_eq!(
                definition.id, *name,
                "known model '{name}' should be a canonical model id, not an alias"
            );
            assert!(
                model_ids.insert(definition.id),
                "duplicate canonical GLiNER model in warm list: {}",
                definition.id
            );
        }

        assert_eq!(model_ids.len(), GLINER_MODELS.len());
    }

    #[test]
    fn backend_cache_helper_reuses_arc_without_rebuilding() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = RwLock::new(AHashMap::default());
        let key = backend_cache_key(Some("balanced"), 4).expect("key");
        let build_count = AtomicUsize::new(0);

        let first = get_or_insert_arc(&cache, key.clone(), || {
            build_count.fetch_add(1, Ordering::Relaxed);
            Ok(7usize)
        })
        .expect("first build");
        let second = get_or_insert_arc(&cache, key, || {
            build_count.fetch_add(1, Ordering::Relaxed);
            Ok(9usize)
        })
        .expect("cache hit");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(*second, 7);
        assert_eq!(build_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn resolves_supported_model_aliases() {
        assert_eq!(resolve_model("fast").expect("fast").id, "gliner_small-v2.5");
        assert_eq!(resolve_model("balanced").expect("balanced").id, "gliner_medium-v2.5");
        assert_eq!(
            resolve_model("gliner_large-v2.5").expect("id").upstream_repo,
            "gliner-community/gliner_large-v2.5"
        );
        assert!(resolve_model("unknown/gliner-model").is_err());
    }

    #[test]
    fn checked_offset_conversion_rejects_u32_overflow() {
        let offset = u32::MAX as usize + 1;
        let err = checked_offset_to_u32(offset, "start", "Alice", "person").expect_err("overflow must fail");

        match err {
            crate::XbergError::Validation { message, source } => {
                assert!(message.contains("GLiNER returned start offset"));
                assert!(message.contains("exceeds the u32 entity offset limit"));
                assert!(source.is_some());
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    #[test]
    fn parses_gliner_checksum_file() {
        let checksums = parse_checksums(
            r#"
            # generated checksums
            AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA  ./models/gliner_small-v2.5/span/fp32/model.onnx
            bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  models/gliner_small-v2.5/span/fp32/tokenizer.json
            "#,
        )
        .expect("checksums");

        assert_eq!(
            required_checksum(&checksums, "models/gliner_small-v2.5/span/fp32/model.onnx").expect("model"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            required_checksum(&checksums, "models/gliner_small-v2.5/span/fp32/tokenizer.json").expect("tokenizer"),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert!(required_checksum(&checksums, "missing/model.onnx").is_err());
    }

    #[test]
    fn rejects_invalid_gliner_checksum_file() {
        assert!(parse_checksums("not-a-sha256 models/model.onnx").is_err());
        assert!(parse_checksums("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
    }

    /// Smoke test — downloads the real model and runs one inference.
    /// Excluded from normal CI; run with:
    ///   cargo test -p xberg --features ner-onnx,ner --lib ner::gline -- --ignored
    #[ignore]
    #[tokio::test]
    async fn smoke_test_real_inference() {
        let backend = GlineBackend::new(None).expect("GlineBackend::new failed");
        let entities = backend
            .detect("Elon Musk founded SpaceX in Hawthorne, California.", &[])
            .await
            .expect("detect failed");
        assert!(!entities.is_empty(), "expected at least one entity");
        let texts: Vec<&str> = entities.iter().map(|entity| entity.text.as_str()).collect();
        assert!(
            texts.contains(&"Elon Musk") || texts.contains(&"SpaceX"),
            "expected at least one known entity, got: {texts:?}"
        );
    }
}
