//! NER (named-entity recognition) configuration.
//!
//! When `ExtractionConfig::ner` is `Some`, the NER post-processor runs after
//! extraction and populates [`ExtractedDocument::entities`](crate::types::ExtractedDocument::entities).

use crate::types::entity::EntityCategory;
use serde::{Deserialize, Serialize};

/// Configuration for the NER post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
#[derive(Default)]
pub struct NerConfig {
    /// Backend that runs the entity detection.
    #[serde(default)]
    pub backend: NerBackendKind,
    /// Entity categories to detect. Defaults to a sensible PERSON/ORG/LOCATION/EMAIL set
    /// when empty.
    #[serde(default)]
    pub categories: Vec<EntityCategory>,
    /// Override the default model — only used by [`NerBackendKind::Onnx`].
    /// `None` lets the backend pick its pinned default xberg GLiNER model alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional LLM configuration — only used by [`NerBackendKind::Llm`]. Token usage
    /// for LLM backends is recorded in `ExtractedDocument::llm_usage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<super::llm::LlmConfig>,
    /// Arbitrary user-supplied entity labels for zero-shot detection.
    ///
    /// `xberg-gliner` natively supports zero-shot inference over caller-supplied
    /// labels. The LLM backend also honours these
    /// labels by including them in the structured-output schema. Custom labels
    /// surface as [`EntityCategory::Custom`] in the resulting `Entity` stream.
    ///
    /// Use this when you need domain-specific entity types (e.g. `"Treatment"`,
    /// `"Product"`, `"Vessel"`) without forking GLiNER's taxonomy.
    #[serde(default)]
    pub custom_labels: Vec<String>,
}

/// NER backend selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NerBackendKind {
    /// `xberg-gliner` ONNX inference. Requires `ner-onnx` feature. Models
    /// download lazily from `xberg-io/gliner-models`.
    #[default]
    Onnx,
    /// liter-llm zero-shot NER via structured-output prompts. Requires `ner-llm`
    /// feature. Useful when domain-specific categories outstrip the ONNX taxonomy.
    Llm,
}
