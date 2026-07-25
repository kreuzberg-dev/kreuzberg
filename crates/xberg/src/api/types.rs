//! API request and response types.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tower::util::BoxCloneService;

use crate::{ExtractionConfig, XbergError, service::ExtractionRequest, types::ExtractedDocument};

/// API server size limit configuration.
///
/// Controls maximum sizes for request bodies and multipart uploads.
/// Default limits are set to 100 MB to accommodate typical document processing workloads.
///
/// # Default Values
///
/// - `max_request_body_bytes`: 100 MB (104,857,600 bytes)
/// - `max_multipart_field_bytes`: 100 MB (104,857,600 bytes)
///
/// # Configuration via Environment Variables
///
/// You can override the defaults using these environment variables:
///
/// ```bash
/// # Modern approach (in bytes):
/// export XBERG_MAX_REQUEST_BODY_BYTES=104857600     # 100 MB
/// export XBERG_MAX_MULTIPART_FIELD_BYTES=104857600  # 100 MB
/// ```
///
/// # Examples
///
/// ```
/// use xberg::api::ApiSizeLimits;
///
/// // Default limits (100 MB)
/// let limits = ApiSizeLimits::default();
///
/// // Custom limits (5 GB for both)
/// let limits = ApiSizeLimits {
///     max_request_body_bytes: 5 * 1024 * 1024 * 1024,
///     max_multipart_field_bytes: 5 * 1024 * 1024 * 1024,
/// };
///
/// // Very large documents (100 GB total, 50 GB per file)
/// let limits = ApiSizeLimits {
///     max_request_body_bytes: 100 * 1024 * 1024 * 1024,
///     max_multipart_field_bytes: 50 * 1024 * 1024 * 1024,
/// };
/// ```
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy)]
pub struct ApiSizeLimits {
    /// Maximum size of the entire request body in bytes.
    ///
    /// This applies to the total size of all uploaded files and form data
    /// in a single request. Default: 100 MB (104,857,600 bytes).
    pub max_request_body_bytes: usize,

    /// Maximum size of a single multipart field in bytes.
    ///
    /// This applies to individual files in a multipart upload.
    /// Default: 100 MB (104,857,600 bytes).
    pub max_multipart_field_bytes: usize,
}

impl Default for ApiSizeLimits {
    fn default() -> Self {
        Self {
            max_request_body_bytes: 100 * 1024 * 1024,
            max_multipart_field_bytes: 100 * 1024 * 1024,
        }
    }
}

impl ApiSizeLimits {
    /// Create new size limits with custom values.
    ///
    /// # Arguments
    ///
    /// * `max_request_body_bytes` - Maximum total request size in bytes
    /// * `max_multipart_field_bytes` - Maximum individual file size in bytes
    pub(crate) fn new(max_request_body_bytes: usize, max_multipart_field_bytes: usize) -> Self {
        Self {
            max_request_body_bytes,
            max_multipart_field_bytes,
        }
    }

    /// Create size limits from MB values (convenience method).
    ///
    /// # Arguments
    ///
    /// * `max_request_body_mb` - Maximum total request size in megabytes
    /// * `max_multipart_field_mb` - Maximum individual file size in megabytes
    ///
    /// # Examples
    ///
    /// ```
    /// use xberg::api::ApiSizeLimits;
    ///
    /// // 50 MB limits
    /// let limits = ApiSizeLimits::from_mb(50, 50);
    /// ```
    #[cfg(test)]
    pub(crate) fn from_mb(max_request_body_mb: usize, max_multipart_field_mb: usize) -> Self {
        Self {
            max_request_body_bytes: max_request_body_mb * 1024 * 1024,
            max_multipart_field_bytes: max_multipart_field_mb * 1024 * 1024,
        }
    }
}

/// Plugin status information in health response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PluginStatus {
    /// Number of registered OCR backends
    pub ocr_backends_count: usize,
    /// Names of registered OCR backends
    pub ocr_backends: Vec<String>,
    /// Number of registered document extractors
    pub extractors_count: usize,
    /// Number of registered post-processors
    pub post_processors_count: usize,
}

/// Health check response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct HealthResponse {
    /// Health status
    #[cfg_attr(feature = "api", schema(example = "healthy"))]
    pub status: String,
    /// API version
    #[cfg_attr(feature = "api", schema(example = "0.8.0"))]
    pub version: String,
    /// Plugin status (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<PluginStatus>,
}

/// Server information response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct InfoResponse {
    /// API version
    #[cfg_attr(feature = "api", schema(example = "0.8.0"))]
    pub version: String,
    /// Whether using Rust backend
    pub rust_backend: bool,
}

/// Error response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ErrorResponse {
    /// Error type name
    #[cfg_attr(feature = "api", schema(example = "ValidationError"))]
    pub error_type: String,
    /// Error message
    #[cfg_attr(feature = "api", schema(example = "Invalid input provided"))]
    pub message: String,
    /// Stack trace (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
    /// HTTP status code
    #[cfg_attr(feature = "api", schema(example = 400))]
    pub status_code: u16,
}

/// API server state.
///
/// Holds the default extraction configuration loaded from config file
/// (via discovery or explicit path). Per-request configs override these defaults.
#[cfg_attr(alef, alef(skip))]
#[derive(Clone)]
pub struct ApiState {
    /// Default extraction configuration
    pub default_config: Arc<ExtractionConfig>,
    /// Tower service for extraction requests.
    ///
    /// Wrapped in `Arc<Mutex>` because `BoxCloneService` is `Send` but not `Sync`,
    /// while `ApiState` must be `Clone + Sync` for Axum's state requirement.
    /// The lock is held only long enough to clone the service.
    pub extraction_service: Arc<Mutex<BoxCloneService<ExtractionRequest, ExtractedDocument, XbergError>>>,
    /// In-memory job store for async extraction polling.
    #[cfg(feature = "api")]
    pub job_store: Arc<super::jobs::JobStore>,
}

/// Response from `POST /extract-async`: a job identifier the client polls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct AsyncJobResponse {
    /// Unique ID to pass to `GET /jobs/{job_id}`.
    pub job_id: String,
}

/// The state of an async extraction job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// The job has been accepted but not yet started.
    Pending,
    /// The job is currently being processed.
    Running,
    /// The job completed successfully.
    Completed,
    /// The job terminated with an error.
    Failed,
    /// The job was cancelled via `DELETE /jobs/{job_id}`.
    Cancelled,
}

/// The status of an async extraction job returned by `GET /jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct JobStatus {
    /// Unique identifier of the job.
    pub job_id: String,
    /// Current lifecycle state of the job.
    pub state: JobState,
    /// ISO 8601 timestamp when the job was created.
    pub created_at: String,
    /// ISO 8601 timestamp of the last state change.
    pub updated_at: String,
    /// The extraction result, present only when `state == completed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error message, present only when `state == failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response from `GET /jobs/{job_id}`.
pub type JobStatusResponse = JobStatus;

/// Cache statistics response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct CacheStatsResponse {
    /// Cache directory path
    #[cfg_attr(feature = "api", schema(example = "/tmp/xberg-cache"))]
    pub directory: String,
    /// Total number of cache files
    pub total_files: usize,
    /// Total cache size in MB
    pub total_size_mb: f64,
    /// Available disk space in MB
    pub available_space_mb: f64,
    /// Age of oldest file in days
    pub oldest_file_age_days: f64,
    /// Age of newest file in days
    pub newest_file_age_days: f64,
}

/// Xberg-managed cache clear response.
///
/// Shared Hugging Face Hub cache files are not included in this operation.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct CacheClearResponse {
    /// Xberg-managed cache directory path
    #[cfg_attr(feature = "api", schema(example = "/tmp/xberg-cache"))]
    pub directory: String,
    /// Number of files removed
    pub removed_files: usize,
    /// Space freed in MB
    pub freed_mb: f64,
}

/// Version response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct VersionResponse {
    /// Xberg version string
    #[cfg_attr(feature = "api", schema(example = "0.8.0"))]
    pub version: String,
}

/// MIME type detection response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct DetectResponse {
    /// Detected MIME type
    #[cfg_attr(feature = "api", schema(example = "application/pdf"))]
    pub mime_type: String,
    /// Original filename (if provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Model manifest entry for cache management.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ManifestEntryResponse {
    /// Relative path within the cache directory
    #[cfg_attr(feature = "api", schema(example = "paddle-ocr/det/model.onnx"))]
    pub relative_path: String,
    /// SHA256 checksum of the model file
    pub sha256: String,
    /// Expected file size in bytes
    pub size_bytes: u64,
    /// HuggingFace source URL for downloading
    pub source_url: String,
}

/// Model manifest response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ManifestResponse {
    /// Xberg version
    #[cfg_attr(feature = "api", schema(example = "0.8.0"))]
    pub xberg_version: String,
    /// Total size of all models in bytes
    pub total_size_bytes: u64,
    /// Number of models in the manifest
    pub model_count: usize,
    /// Individual model entries
    pub models: Vec<ManifestEntryResponse>,
}

/// Cache warm request.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct WarmRequest {
    /// Download all embedding model presets
    #[serde(default)]
    pub all_embeddings: bool,
    /// Specific embedding model preset to download
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Download the default GLiNER NER model
    #[serde(default)]
    pub ner: bool,
    /// Download every known GLiNER NER model
    #[serde(default)]
    pub all_ner_models: bool,
    /// Specific GLiNER NER model alias or catalog id to download
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ner_model: Option<String>,
}

/// Cache warm response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct WarmResponse {
    /// Xberg-managed cache directory used.
    ///
    /// Hugging Face artifacts remain in the standard HF cache, and their
    /// resolved paths are included in `downloaded`.
    pub cache_dir: String,
    /// Models that were downloaded
    pub downloaded: Vec<String>,
    /// Models that were already cached
    pub already_cached: Vec<String>,
}

/// OpenWebUI "External" engine response format.
///
/// Returned by `PUT /process` for the OpenWebUI external document loader.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OpenWebDocumentResponse {
    /// Extracted text content
    pub page_content: String,
    /// Document metadata
    pub metadata: OpenWebDocumentMetadata,
}

/// Metadata for the OpenWebUI external document loader response.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct OpenWebDocumentMetadata {
    /// Original filename
    #[cfg_attr(feature = "api", schema(example = "document.pdf"))]
    pub source: String,
}

/// OpenWebUI "Docling" engine response format.
///
/// Returned by `POST /v1/convert/file` for docling-serve compatibility.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct DoclingCompatResponse {
    /// Converted document content
    pub document: DoclingCompatDocument,
    /// Processing status
    #[cfg_attr(feature = "api", schema(example = "success"))]
    pub status: String,
}

/// Document content in the docling-serve response format.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct DoclingCompatDocument {
    /// Markdown content of the converted document
    pub md_content: String,
}
