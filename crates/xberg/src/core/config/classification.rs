//! Page-classification configuration.
//!
//! When `ExtractionConfig::page_classification` is `Some`, the page-classification
//! post-processor runs at the Middle stage and populates
//! [`ExtractedDocument::page_classifications`](crate::types::ExtractedDocument::page_classifications).

use serde::{Deserialize, Serialize};

/// Configuration for the page-classification post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct PageClassificationConfig {
    /// Minijinja prompt template. Receives `{{ labels }}` (joined list), `{{ page_text }}`
    /// and `{{ multi_label }}` variables. `None` lets the backend pick a sensible default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    /// The set of labels the classifier may emit. Must contain at least one entry.
    pub labels: Vec<String>,
    /// Allow multiple labels per page. Single-label mode returns at most one label.
    #[serde(default)]
    pub multi_label: bool,
    /// LLM configuration used for classification.
    pub llm: super::llm::LlmConfig,
}
