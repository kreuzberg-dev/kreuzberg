/// UTF-8 validation and safe decoding helpers.
pub mod utf8_validation;

#[cfg(any(feature = "office", feature = "email"))]
/// Windows codepage number to `encoding_rs` encoding mapping.
pub(crate) mod windows_codepage;

#[cfg(feature = "quality")]
/// OCR quality scoring: noise detection, confidence aggregation, and artifact removal.
pub mod quality;

#[cfg(feature = "quality")]
/// String utilities: mojibake repair, encoding detection, safe truncation.
pub mod string_utils;

#[cfg(feature = "quality")]
/// Token-level text reduction pipeline for summarizing or compressing document content.
pub mod token_reduction;

#[cfg(feature = "quality")]
pub mod quality_processor;

#[cfg(feature = "quality")]
pub use quality_processor::QualityProcessor;

#[cfg(feature = "quality")]
pub use token_reduction::{ReductionLevel, TokenReductionConfig};

#[cfg(feature = "classification")]
pub mod classification;

#[cfg(not(feature = "classification"))]
/// Page-classification API stub (classification feature not enabled on this target).
pub mod classification {
    use crate::{ChunkClassificationConfig, ClassificationLabel, ExtractedDocument, PageClassificationConfig, Result};

    /// Classify pages in an extraction result.
    pub async fn classify_pages(_result: &mut ExtractedDocument, _config: &PageClassificationConfig) -> Result<()> {
        Err(crate::XbergError::Other(
            "classification feature not available on this target".into(),
        ))
    }

    /// Classify a free-form text string. Stub form mirrors the real
    /// `classify_text` signature so language bindings keep compiling on
    /// targets without the `classification` feature (e.g. `android-target`,
    /// `wasm-target`).
    pub async fn classify_text(_text: &str, _config: &PageClassificationConfig) -> Result<Vec<ClassificationLabel>> {
        Err(crate::XbergError::Other(
            "classification feature not available on this target".into(),
        ))
    }

    /// Classify chunks in an extraction result. Stub form mirrors the real
    /// `classify_chunks` signature so language bindings keep compiling on
    /// targets without the `classification` feature.
    pub async fn classify_chunks(_result: &mut ExtractedDocument, _config: &ChunkClassificationConfig) -> Result<()> {
        Err(crate::XbergError::Other(
            "classification feature not available on this target".into(),
        ))
    }
}

#[cfg(feature = "ner")]
pub mod ner;

#[cfg(not(feature = "ner"))]
/// Named-entity recognition API stub (ner feature not enabled on this target).
pub mod ner {
    use crate::Result;
    use std::path::PathBuf;

    /// NER backend trait (stub for Android x86_64).
    pub trait NerBackend: Send + Sync {}

    /// Download a NER model into the xberg cache.
    pub fn download_model(_name: &str, _cache_dir: Option<PathBuf>) -> Result<PathBuf> {
        Err(crate::XbergError::Other(
            "ner feature not available on this target".into(),
        ))
    }

    /// Default NER model identifier.
    pub fn default_model_name() -> &'static str {
        "gliner-stub"
    }

    /// All NER models xberg knows about.
    pub fn known_models() -> Vec<&'static str> {
        vec![]
    }
}

#[cfg(feature = "redaction")]
pub mod redaction;
#[cfg(feature = "summarization")]
pub mod summarization;

#[cfg(feature = "translation")]
pub mod translation;

#[cfg(not(feature = "translation"))]
/// Translation API stub (translation feature not enabled on this target).
pub mod translation {
    use crate::{ExtractedDocument, Result, TranslationConfig};

    /// Translate an extraction result.
    pub async fn translate_result(_result: &mut ExtractedDocument, _config: &TranslationConfig) -> Result<()> {
        Err(crate::XbergError::Other(
            "translation feature not available on this target".into(),
        ))
    }
}

#[cfg(feature = "markdown-footnotes")]
/// Markdown footnote and citation parsing.
pub mod markdown_footnotes;
