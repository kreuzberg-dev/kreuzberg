//! Translation configuration.
//!
//! When `ExtractionConfig::translation` is `Some`, the translation post-processor runs
//! at the Middle stage and populates
//! [`ExtractedDocument::translation`](crate::types::ExtractedDocument::translation).

use serde::{Deserialize, Serialize};

/// Configuration for the translation post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct TranslationConfig {
    /// BCP-47 language tag for the target language (e.g. `"de"`, `"fr-CA"`).
    pub target_lang: String,
    /// Optional explicit source language. `None` asks the backend to auto-detect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_lang: Option<String>,
    /// Translate the formatted (Markdown/HTML) rendition alongside plain text when
    /// `formatted_content` is present.
    #[serde(default)]
    pub preserve_markup: bool,
    /// LLM configuration used for translation.
    pub llm: super::llm::LlmConfig,
}
