//! VLM image-captioning configuration.
//!
//! When `ExtractionConfig::captioning` is `Some`, the captioning post-processor runs at
//! the Middle stage, iterates `ExtractedDocument::images`, and populates
//! [`ExtractedImage::caption`](crate::types::ExtractedImage::caption) for each image whose
//! pixel area exceeds `min_image_area`.

use serde::{Deserialize, Serialize};

/// Configuration for the VLM captioning post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct CaptioningConfig {
    /// LLM configuration used for the VLM call.
    pub llm: super::llm::LlmConfig,
    /// Optional custom caption prompt. `None` uses the default `RegionKind::Caption`
    /// prompt that ships with `crate::llm::region_extractor`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Skip images whose `width * height` is below this threshold (in pixels).
    /// Default `1_000` filters out icons and decorations.
    #[serde(default = "default_min_image_area")]
    pub min_image_area: u32,
}

fn default_min_image_area() -> u32 {
    1_000
}
