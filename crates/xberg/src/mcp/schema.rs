//! MCP response DTO types with JSON Schema support.
//!
//! These types are used as structured output for MCP tool calls,
//! providing both human-readable text content and machine-parseable
//! structured data in a single response.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// Structured output for unified extraction.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtractionResult {
    /// Extraction results in discovery order.
    #[schemars(description = "Extraction results in discovery order")]
    pub results: Vec<serde_json::Value>,
    /// Non-fatal per-input errors.
    #[serde(default)]
    #[schemars(description = "Non-fatal per-input errors")]
    pub errors: Vec<serde_json::Value>,
    /// Aggregate extraction counts.
    #[schemars(description = "Aggregate extraction counts")]
    pub summary: ExtractionSummaryOutput,
    /// Final URLs reached after redirects during URL ingestion.
    #[serde(default)]
    #[schemars(description = "Final URLs reached after redirects during URL ingestion")]
    pub crawl_final_urls: Vec<String>,
    /// Total redirects followed while fetching or crawling URLs.
    #[serde(default)]
    #[schemars(description = "Total redirects followed while fetching or crawling URLs")]
    pub crawl_redirect_count: usize,
    /// Unique normalized URLs discovered by crawls.
    #[serde(default)]
    #[schemars(description = "Unique normalized URLs discovered by crawls")]
    pub crawl_unique_normalized_urls: Vec<String>,
}

/// Structured summary for unified extraction.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExtractionSummaryOutput {
    /// Number of inputs submitted by the caller.
    pub inputs: usize,
    /// Number of extraction results produced.
    pub results: usize,
    /// Number of per-input errors.
    pub errors: usize,
    /// Number of remote HTTP(S) URLs resolved.
    pub remote_urls: usize,
    /// Number of HTML pages crawled or scraped.
    pub pages_crawled: usize,
    /// Number of downloaded non-HTML documents extracted from URLs.
    pub documents_downloaded: usize,
}

/// Structured output for MIME type detection.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DetectMimeTypeOutput {
    /// Detected MIME type string.
    #[schemars(description = "Detected MIME type string")]
    pub mime_type: String,
}

/// Structured output listing all supported formats.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ListFormatsOutput {
    /// List of supported document formats with extension and MIME type.
    #[schemars(description = "List of supported document formats")]
    pub formats: Vec<serde_json::Value>,
}

/// Structured output for library version information.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VersionOutput {
    /// Xberg library version string.
    #[schemars(description = "Xberg library version string")]
    pub version: String,
}

/// Structured output for cache statistics.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheStatsOutput {
    /// Absolute path to the cache directory.
    #[schemars(description = "Absolute path to the cache directory")]
    pub directory: String,
    /// Total number of cached files.
    #[schemars(description = "Total number of cached files")]
    pub total_files: u64,
    /// Total cache size in megabytes.
    #[schemars(description = "Total cache size in megabytes")]
    pub total_size_mb: f64,
    /// Available disk space in megabytes.
    #[schemars(description = "Available disk space in megabytes")]
    pub available_space_mb: f64,
}

/// Structured output for cache clearing.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheClearOutput {
    /// Absolute path to the cache directory that was cleared.
    #[schemars(description = "Absolute path to the cache directory that was cleared")]
    pub directory: String,
    /// Number of files removed.
    #[schemars(description = "Number of files removed")]
    pub removed_files: u64,
    /// Disk space freed in megabytes.
    #[schemars(description = "Disk space freed in megabytes")]
    pub freed_mb: f64,
}

/// Structured output for model cache warming.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheWarmOutput {
    /// Absolute path to the Xberg-managed cache directory.
    #[schemars(description = "Absolute path to the Xberg-managed cache directory")]
    pub cache_dir: String,
    /// Labels of models confirmed available after this call.
    #[schemars(description = "Labels of models confirmed available after this call")]
    pub available: Vec<String>,
    /// Labels of models confirmed to have been newly downloaded during this call.
    ///
    /// Some model managers expose only an idempotent warm operation, not whether it
    /// performed network I/O. Those models appear in `available` but not here.
    #[schemars(description = "Labels of models confirmed newly downloaded during this call")]
    pub downloaded: Vec<String>,
    /// Labels of models confirmed to have already been present in the cache.
    ///
    /// Some model managers cannot distinguish a cache hit from a download. Those
    /// models appear in `available` but not here.
    #[schemars(description = "Labels of models confirmed already present in the cache")]
    pub already_cached: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ExtractionResult;

    #[test]
    fn should_not_require_optional_extraction_fields_in_output_schema() {
        let schema = rmcp::handler::server::common::schema_for_output::<ExtractionResult>();

        let required: BTreeSet<String> = schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("schema must declare a top-level `required` array")
            .iter()
            .map(|entry| entry.as_str().expect("required entries must be strings").to_string())
            .collect();

        let expected: BTreeSet<String> = ["results", "summary"].into_iter().map(String::from).collect();

        assert_eq!(required, expected);
    }
}

/// Structured output for the model manifest.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CacheManifestOutput {
    /// Xberg library version.
    #[schemars(description = "Xberg library version")]
    pub xberg_version: String,
    /// Number of model files in the manifest.
    #[schemars(description = "Number of model files in the manifest")]
    pub model_count: usize,
    /// Total size of all model files in bytes.
    #[schemars(description = "Total size of all model files in bytes")]
    pub total_size_bytes: u64,
    /// Model file entries with name, size, and checksum.
    #[schemars(description = "Model file entries")]
    pub models: Vec<serde_json::Value>,
}
