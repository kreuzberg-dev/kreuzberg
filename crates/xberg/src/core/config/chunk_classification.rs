//! Chunk-classification configuration.
//!
//! When `ExtractionConfig::chunk_classification` is `Some`, the chunk-classification
//! post-processor runs at the Middle stage (after chunking has populated
//! `ExtractedDocument::chunks`) and populates
//! [`ChunkMetadata::classifications`](crate::types::ChunkMetadata::classifications)
//! on every chunk.

use serde::{Deserialize, Serialize};

/// Default number of chunks grouped into a single classification request.
///
/// Batching amortizes the fixed cost of the (potentially large) definitions
/// block, which is repeated verbatim in every request, across more chunks.
pub const DEFAULT_BATCH_SIZE: usize = 10;

/// Default number of concurrent in-flight classification batch requests.
///
/// Bounds concurrency against the configured LLM provider to avoid
/// rate-limit failures on large documents.
pub const DEFAULT_MAX_CONCURRENCY: usize = 4;

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

fn default_max_concurrency() -> usize {
    DEFAULT_MAX_CONCURRENCY
}

/// A single labeled definition the chunk classifier may emit.
///
/// Unlike `PageClassificationConfig::labels` (bare label names), chunk
/// classification targets potentially large domain taxonomies where every
/// label carries its own semantic description, letting the LLM disambiguate
/// similarly named labels without relying on the label string alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct ChunkClassificationDefinition {
    /// Label name returned in `ChunkMetadata::classifications`.
    pub label: String,
    /// Semantic description of when this label applies. Injected verbatim into
    /// the classification prompt next to the label name.
    pub description: String,
}

/// Configuration for the chunk-classification post-processor.
///
/// Chunk classification is always multi-label: a chunk may match zero, one, or
/// many of the configured definitions. This is the chunk-level equivalent of
/// [`super::PageClassificationConfig`], but scoped to individual chunks
/// (`ExtractedDocument::chunks`) rather than whole pages, and built for large
/// taxonomies where each label needs its own description rather than a bare name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct ChunkClassificationConfig {
    /// Minijinja prompt template. Receives `{{ definitions }}` (rendered label +
    /// description list) and `{{ chunks }}` (a numbered list of chunk texts in
    /// the current batch) variables. `None` lets the backend pick a sensible
    /// default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    /// The set of label definitions the classifier may emit. Must contain at
    /// least one entry.
    pub definitions: Vec<ChunkClassificationDefinition>,
    /// LLM configuration used for classification.
    pub llm: super::llm::LlmConfig,
    /// Number of chunks batched into a single LLM request.
    ///
    /// Larger batches amortize the fixed prompt cost (definitions block) across
    /// more chunks, at the risk of exceeding the model's context window for
    /// very large taxonomies or chunk texts. Defaults to [`DEFAULT_BATCH_SIZE`].
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Maximum number of in-flight batch requests.
    ///
    /// Bounds concurrency against the configured LLM provider. Defaults to
    /// [`DEFAULT_MAX_CONCURRENCY`].
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}
