//! Named-entity recognition (NER).
//!
//! Shared by:
//! - the NER post-processor at `crate::plugins::processor::builtin::ner` (populates
//!   [`ExtractedDocument::entities`](crate::types::ExtractedDocument::entities))
//! - the redaction engine at `crate::text::redaction::engine` (consumes the same
//!   `Entity` stream to redact PERSON / ORGANIZATION / LOCATION mentions that the
//!   pure-Rust pattern engine cannot detect).
//!
//! Backends implement the [`NerBackend`] trait. Two are bundled:
//!
//! - [`gline::GlineBackend`] under `#[cfg(feature = "ner-onnx")]` — local ONNX
//!   inference via `xberg-gliner`. Models download lazily from the
//!   `xberg-io/gliner-models` Hugging Face repository via
//!   `crate::model_download`.
//! - [`llm::LlmBackend`] under `#[cfg(feature = "ner-llm")]` — liter-llm with a
//!   structured-output schema. Used when categories outstrip the ONNX taxonomy.
//!
//! Every backend has a bounded input length, so both bundled backends window
//! long input through [`offsets::split_into_windows`] rather than letting the
//! model silently truncate — an undetected entity is PII that never gets
//! redacted (xberg-io/xberg#262).

#![cfg(feature = "ner")]

pub mod backend;
#[cfg(feature = "ner-candle-backend")]
pub mod candle;
#[cfg(feature = "ner-onnx")]
pub mod gline;
#[cfg(all(feature = "ner-llm", not(all(target_os = "android", target_arch = "x86_64"))))]
pub mod llm;
pub mod offsets;

pub use backend::NerBackend;

use crate::Result;
use crate::types::entity::Entity;

use std::path::PathBuf;

/// Eagerly download a NER model into the Hugging Face Hub cache.
///
/// `name` is a supported xberg GLiNER alias or catalog id. The CLI flag
/// `xberg cache warm --ner` delegates here. `cache_dir`, when provided, is a
/// custom Hugging Face Hub cache root; otherwise the standard `HF_HUB_CACHE`,
/// `HF_HOME`, and platform defaults apply.
#[cfg(feature = "ner-onnx")]
pub fn download_model(name: &str, cache_dir: Option<PathBuf>) -> crate::Result<PathBuf> {
    gline::download_model(name, cache_dir)
}

#[cfg(not(feature = "ner-onnx"))]
pub fn download_model(_name: &str, _cache_dir: Option<PathBuf>) -> crate::Result<PathBuf> {
    Err(crate::XbergError::Other(
        "ner-onnx feature not available on this target".into(),
    ))
}

/// Pinned default NER model identifier.
#[cfg(feature = "ner-onnx")]
pub fn default_model_name() -> &'static str {
    gline::DEFAULT_MODEL_NAME
}

#[cfg(not(feature = "ner-onnx"))]
pub fn default_model_name() -> &'static str {
    "gliner-stub"
}

/// All NER models xberg knows about (used by `--all-ner-models`).
#[cfg(feature = "ner-onnx")]
pub fn known_models() -> &'static [&'static str] {
    gline::KNOWN_MODELS
}

#[cfg(not(feature = "ner-onnx"))]
pub fn known_models() -> &'static [&'static str] {
    &[]
}

/// Expected GLiNER cache artifacts for manifest tooling.
#[cfg(feature = "ner-onnx")]
#[cfg_attr(alef, alef(skip))]
pub fn manifest() -> Vec<gline::GlinerManifestEntry> {
    gline::manifest()
}

/// Detect named entities in the given text using the provided backend.
///
/// Identifies entities such as persons, organizations, locations, dates, and more
/// based on the backend's capabilities and the categories requested.
///
/// # Arguments
///
/// * `text` - The input text to analyze.
/// * `backend` - The NER backend implementation to use (either ONNX-based GLiNER or LLM-driven).
/// * `categories` - Entity categories to detect. If empty, the backend returns all entities it can identify.
///
/// # Returns
///
/// A vector of detected `Entity` objects in source byte-offset order.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::types::entity::EntityCategory;
/// use xberg::text::ner::detect_entities;
/// use xberg::core::config::LlmConfig;
/// use xberg::LlmBackend;
///
/// # async fn example() -> xberg::Result<()> {
/// let backend = LlmBackend::new(LlmConfig::default());
/// let categories = vec![EntityCategory::Person, EntityCategory::Organization];
/// let entities = detect_entities("Alice works at Acme Corp.", &backend, &categories).await?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub async fn detect_entities(
    text: &str,
    backend: &dyn NerBackend,
    categories: &[crate::types::entity::EntityCategory],
) -> Result<Vec<Entity>> {
    backend.detect(text, categories).await
}
