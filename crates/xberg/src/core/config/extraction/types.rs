//! Feature-specific configuration types for extraction.
//!
//! This module contains configuration structs for specific extraction features:
//! - Image extraction and processing
//! - Token reduction
//! - Language detection
//! - Batch extraction items

use serde::{Deserialize, Serialize};
#[cfg(all(test, feature = "tokio-runtime", not(target_arch = "wasm32")))]
use std::path::PathBuf;

use crate::types::ExtractedDocument;

/// Target format for re-encoding extracted images.
///
/// Controls whether and how extracted images are normalised to a uniform
/// container format before being returned in `ExtractedDocument.images`.
/// The default (`Native`) preserves the format produced by each extractor
/// without any additional encode pass.
///
/// Callers that need uniform output — e.g. cloud pipelines that always store
/// WebP thumbnails — set this once on `ImageExtractionConfig.output_format`
/// rather than re-encoding downstream.
///
/// # Serde shape
///
/// Uses a tagged enum: `{"type": "native"}`, `{"type": "png"}`,
/// `{"type": "jpeg", "quality": 90}`, etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageOutputFormat {
    /// Preserve whatever format the extractor produced (default).
    ///
    /// No re-encode pass is performed. `ExtractedImage.format` reflects the
    /// source format: JPEG for embedded PDF images, PNG for rasterised content,
    /// or the native container format from office documents.
    #[default]
    Native,

    /// Re-encode all extracted images as PNG (lossless).
    Png,

    /// Re-encode all extracted images as JPEG at the given quality level.
    ///
    /// `quality` must be in `1..=100`. Values outside this range are clamped
    /// and a warning is emitted. Higher values produce larger files with less
    /// artefacting; 85 is a reasonable default.
    Jpeg {
        /// JPEG quality (1–100, default 85).
        #[serde(default = "default_jpeg_quality")]
        quality: u8,
    },

    /// Re-encode all extracted images as WebP at the given quality level.
    ///
    /// `quality` must be in `1..=100`. Values outside this range are clamped
    /// and a warning is emitted. 80 is a reasonable default.
    Webp {
        /// WebP quality (1–100, default 80).
        #[serde(default = "default_webp_quality")]
        quality: u8,
    },

    /// Re-encode all extracted images as HEIF/HEIC at the given quality level.
    ///
    /// `quality` must be in `1..=100`. Values outside this range are clamped
    /// and a warning is emitted. 80 is a reasonable default.
    ///
    /// The encode path requires the `heic` feature; on builds without it, selecting
    /// this variant returns an `EncodeFailed` warning and leaves the image untouched.
    Heif {
        /// HEIF quality (1–100, default 80).
        #[serde(default = "default_heif_quality")]
        quality: u8,
    },

    /// Output pure-vector SVG. Lossless. Raster sources are not re-encoded
    /// (a warning is emitted and the image bytes are left untouched).
    ///
    /// When the source is already SVG, the bytes are passed through the
    /// `usvg` sanitizer (strips external hrefs, JS event handlers, and
    /// `foreignObject` elements) when [`SvgOptions::sanitize`] is `true`.
    ///
    /// Requires the `svg` feature.
    #[cfg(feature = "svg")]
    Svg,
}

const fn default_jpeg_quality() -> u8 {
    85
}

const fn default_webp_quality() -> u8 {
    80
}

const fn default_heif_quality() -> u8 {
    80
}

/// SVG-specific configuration for the image-encode pipeline.
///
/// Applies when the source image is SVG or when the output format is set to
/// [`ImageOutputFormat::Svg`].  Available when the `svg` feature is active.
///
/// Used via [`ImageExtractionConfig::svg`].
#[cfg(feature = "svg")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SvgOptions {
    /// Run SVG bytes through `usvg` sanitization (strips external `href` attributes,
    /// JavaScript event handlers, and `foreignObject` elements) even when the
    /// output format is `Native`.  Defaults to `true`.
    pub sanitize: bool,

    /// Target DPI when rasterizing SVG to a pixel-based format (PNG, JPEG, WebP,
    /// HEIF).  The tree's viewBox is scaled by `render_dpi / 96.0` before the
    /// pixel buffer is allocated.  Defaults to `96.0` (1× CSS pixel density).
    pub render_dpi: f32,
}

#[cfg(feature = "svg")]
impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            sanitize: true,
            render_dpi: 96.0,
        }
    }
}

/// Source kind for [`ExtractInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExtractInputKind {
    /// Raw in-memory bytes.
    Bytes,
    /// A filesystem path, `file://` URI, or HTTP(S) URL.
    Uri,
}

/// Unified extraction input for all public extraction entry points.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct ExtractInput {
    /// Source kind. `bytes` requires `bytes`; `uri` requires `uri`.
    pub kind: ExtractInputKind,
    /// Raw bytes for `kind = "bytes"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    /// Local path, `file://` URI, or HTTP(S) URL for `kind = "uri"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// MIME type hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Filename hint used for MIME detection and metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Per-input extraction overrides.
    #[cfg_attr(feature = "api", schema(value_type = Option<serde_json::Value>))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<super::FileExtractionConfig>,
}

impl Default for ExtractInput {
    fn default() -> Self {
        Self {
            kind: ExtractInputKind::Uri,
            bytes: None,
            uri: None,
            mime_type: None,
            filename: None,
            config: None,
        }
    }
}

impl ExtractInput {
    /// Build a bytes input with a MIME type and optional filename hint.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>, mime_type: impl Into<String>, filename: Option<String>) -> Self {
        Self {
            kind: ExtractInputKind::Bytes,
            bytes: Some(bytes.into()),
            mime_type: Some(mime_type.into()),
            filename,
            ..Default::default()
        }
    }

    /// Build a URI input from a local path, `file://` URI, or HTTP(S) URL.
    pub fn from_uri(uri: impl Into<String>) -> Self {
        Self {
            kind: ExtractInputKind::Uri,
            uri: Some(uri.into()),
            ..Default::default()
        }
    }
}

/// Non-fatal per-input extraction error captured by [`ExtractionResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ExtractionErrorItem {
    /// Input index in the original request.
    pub index: usize,
    /// Stable numeric error code.
    pub code: u32,
    /// Stable snake_case error kind.
    pub error_type: String,
    /// Best-effort source identifier.
    pub source: String,
    /// Error message.
    pub message: String,
}

/// Summary for a unified extraction call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ExtractionSummary {
    /// Number of inputs submitted by the caller.
    pub inputs: usize,
    /// Number of extraction results produced.
    pub results: usize,
    /// Number of per-input errors.
    pub errors: usize,
    /// Number of URI inputs that resolved to remote HTTP(S) URLs.
    pub remote_urls: usize,
    /// Number of HTML pages crawled or scraped.
    pub pages_crawled: usize,
    /// Number of downloaded non-HTML documents extracted from URLs.
    pub documents_downloaded: usize,
}

/// Unified extraction result envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ExtractionResult {
    /// Extracted documents in discovery order.
    pub results: Vec<ExtractedDocument>,
    /// Non-fatal per-input errors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ExtractionErrorItem>,
    /// Aggregate counts for the operation.
    pub summary: ExtractionSummary,
    /// Final URLs reached after redirects during URL ingestion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crawl_final_urls: Vec<String>,
    /// Total redirects followed while fetching or crawling URLs.
    #[serde(default, skip_serializing_if = "crate::core::config::extraction::types::is_zero")]
    pub crawl_redirect_count: usize,
    /// Unique normalized URLs discovered by crawls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crawl_unique_normalized_urls: Vec<String>,
}

impl ExtractionResult {
    /// Build an output containing one successful result.
    pub fn single(result: ExtractedDocument) -> Self {
        Self {
            results: vec![result],
            summary: ExtractionSummary {
                inputs: 1,
                results: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub(crate) fn refresh_counts(&mut self) {
        self.summary.results = self.results.len();
        self.summary.errors = self.errors.len();
    }
}

pub(crate) fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// URL extraction mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum UrlExtractionMode {
    /// Classify HTTP(S) resources after fetch.
    #[default]
    Auto,
    /// Treat the URI as a single remote document/page.
    Document,
    /// Crawl from the seed URI and extract discovered pages/documents.
    Crawl,
}

/// URL ingestion and crawl configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct UrlExtractionConfig {
    /// URL extraction mode.
    pub mode: UrlExtractionMode,
    /// Crawlberg crawl configuration used for HTTP(S) URL extraction.
    #[cfg(any(feature = "url-ingestion", feature = "url-config-types"))]
    #[cfg_attr(feature = "api", schema(value_type = serde_json::Value))]
    pub crawl: crawlberg::CrawlConfig,
    /// Optional regex filter for document-discovered URLs.
    pub document_url_pattern: Option<String>,
    /// Maximum URLs to follow per extraction result.
    pub max_document_urls_per_result: Option<u32>,
    /// Maximum URLs followed across the whole extraction call.
    pub max_total_urls: Option<u32>,
    /// Allow bare local filesystem path inputs.
    pub allow_local_file_inputs: bool,
    /// Allow local `file://` URI inputs.
    pub allow_file_uris: bool,
}

impl Default for UrlExtractionConfig {
    fn default() -> Self {
        Self {
            mode: UrlExtractionMode::Auto,
            #[cfg(any(feature = "url-ingestion", feature = "url-config-types"))]
            crawl: default_xberg_crawl_config(),
            document_url_pattern: None,
            max_document_urls_per_result: Some(100),
            max_total_urls: Some(1_000),
            allow_local_file_inputs: true,
            allow_file_uris: true,
        }
    }
}

#[cfg(any(feature = "url-ingestion", feature = "url-config-types"))]
fn default_xberg_crawl_config() -> crawlberg::CrawlConfig {
    crawlberg::CrawlConfig {
        max_depth: Some(1),
        max_pages: Some(100),
        max_concurrent: Some(10),
        respect_robots_txt: true,
        soft_http_errors: true,
        stay_on_domain: true,
        allow_subdomains: true,
        document_url_depth: Some(1),
        ..Default::default()
    }
}

/// **Deprecated and inert.** Chunking no longer writes a heading breadcrumb into
/// `content` for either variant of this enum — see the revised design adopted in
/// <https://github.com/xberg-io/xberg/issues/1393>. Setting this field has no
/// effect on chunking output any more. It is kept only so the ~15 alef-generated
/// binding packages that construct it keep compiling; removing it outright is a
/// separate, coordinated breaking change.
///
/// # Why this became inert
///
/// The original design (this enum, plus
/// [`ChunkingConfig::prepend_heading_context`](super::super::processing::ChunkingConfig::prepend_heading_context))
/// let `Content` mode prepend the heading breadcrumb directly into a chunk's
/// `content`. GH#1393's follow-up discussion argued that a single flag on the
/// chunker cannot serve all three retrieval consumers of the same chunk: dense/
/// embedding retrieval wants the breadcrumb inline, but lexical (BM25/TF-IDF) and
/// sparse learned (SPLADE) retrieval are actively harmed by it — SPLADE worse
/// than BM25, because its term-expansion pulls each heading's whole learned
/// neighbourhood (e.g. `"Authentication"` → `auth`, `login`, `credential`,
/// `oauth`) into every chunk of that section, and that damage cannot be
/// corrected by re-indexing since the expansion comes from a pretrained encoder,
/// not the collection being indexed. Mutating `content` also desynced it from
/// `byte_start`/`byte_end` (#1294): `chunk.content.len() != byte_end - byte_start`
/// whenever a breadcrumb had been prepended, so slicing the source document by a
/// chunk's own offsets silently returned different text than `content`.
///
/// The revised design removes the mutation entirely: `chunk.content` now always
/// equals the exact `[byte_start, byte_end)` source span, regardless of this
/// enum's value or `prepend_heading_context`.
///
/// # What to do instead
///
/// Call [`render_heading_breadcrumb`](crate::chunking::render_heading_breadcrumb)
/// explicitly at index time, with a chunk's (always-clean) `content` and its
/// [`heading_context`](crate::types::ChunkMetadata::heading_context) — only for
/// the consumer that wants the breadcrumb inline (typically dense/embedding).
/// BM25 and SPLADE consumers need no special handling: index `chunk.content` as
/// returned. See the [`rag`](crate::chunking::rag) module docs for the full
/// per-consumer guidance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BreadcrumbTarget {
    /// Inert (#1393). Previously prepended the heading breadcrumb into chunk
    /// `content`; no longer has any effect — `content` is left untouched, exactly
    /// like `Metadata`. Kept as the default only for wire/API compatibility.
    #[default]
    Content,
    /// Inert (#1393), and was already a no-op on `content` before this change.
    /// Kept only for backward compatibility, since `Content` is no longer
    /// distinguishable from it.
    Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg(all(test, feature = "tokio-runtime", not(target_arch = "wasm32")))]
pub(crate) struct BatchBytesItem {
    /// The content bytes to extract from
    pub content: Vec<u8>,

    /// MIME type of the content (e.g., "application/pdf", "text/html")
    pub mime_type: String,

    /// Per-item configuration overrides (None uses batch-level defaults)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<super::FileExtractionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg(all(test, feature = "tokio-runtime", not(target_arch = "wasm32")))]
pub(crate) struct BatchFileItem {
    /// Path to the file to extract from
    pub path: PathBuf,

    /// Per-file configuration overrides (None uses batch-level defaults)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<super::FileExtractionConfig>,
}

/// Image extraction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageExtractionConfig {
    /// Extract images from documents
    #[serde(default = "default_true")]
    pub extract_images: bool,

    /// Target DPI for image normalization
    #[serde(default = "default_target_dpi")]
    pub target_dpi: i32,

    /// Maximum dimension for images (width or height)
    #[serde(default = "default_max_dimension")]
    pub max_image_dimension: i32,

    /// Whether to inject image reference placeholders into markdown output.
    /// When `true` (default), image references like `![Image 1](embedded:p1_i0)`
    /// are appended to the markdown. Set to `false` to extract images as data
    /// without polluting the markdown output.
    #[serde(default = "default_true")]
    pub inject_placeholders: bool,

    /// Automatically adjust DPI based on image content
    #[serde(default = "default_true")]
    pub auto_adjust_dpi: bool,

    /// Minimum DPI threshold
    #[serde(default = "default_min_dpi")]
    pub min_dpi: i32,

    /// Maximum DPI threshold
    #[serde(default = "default_max_dpi")]
    pub max_dpi: i32,

    /// Maximum number of image objects to extract per PDF page.
    ///
    /// Some PDFs (e.g. technical diagrams stored as thousands of raster fragments)
    /// can trigger extremely long or indefinite extraction times when every image
    /// object on a dense page is decoded individually via the PDF extractor. Setting this
    /// limit causes xberg to stop collecting individual images once the count
    /// per page reaches the cap and emit a warning instead.
    ///
    /// `None` (default) means no limit — all images are extracted.
    #[serde(default)]
    pub max_images_per_page: Option<u32>,

    /// When `true`, extracted images are classified by kind and grouped
    /// into clusters where they appear to belong to one figure.
    /// Defaults to `false` — opt in explicitly to avoid unexpected ML overhead.
    #[serde(default)]
    pub classify: bool,

    /// When `true`, full-page renders produced during OCR preprocessing are captured
    /// and returned as `ImageKind::PageRaster` entries in `ExtractedDocument.images`.
    ///
    /// **PDF + OCR only.** No rasters are captured for non-PDF inputs or when the
    /// document-level OCR bypass is active (whole-document backend). When OCR is
    /// enabled and this flag is set but the active backend skips per-page rendering,
    /// a `ProcessingWarning` is emitted in `ExtractedDocument.processing_warnings`.
    ///
    /// Defaults to `false`. Enable when downstream consumers need page thumbnails
    /// (e.g. citation previews, visual grounding).
    #[serde(default)]
    pub include_page_rasters: bool,

    /// Run OCR on extracted images and include the recognized text in the document content.
    ///
    /// When `true` (default) and `ExtractionConfig.ocr` is configured, extracted images
    /// are processed with the configured OCR backend. Set to `false` to extract images
    /// without OCR processing, even when OCR is enabled.
    #[serde(default = "default_true")]
    pub run_ocr_on_images: bool,

    /// When `true`, image OCR results are rendered as plain text without the
    /// `![...](...)` markdown placeholder. Only takes effect when `run_ocr_on_images`
    /// is also `true`.
    #[serde(default)]
    pub ocr_text_only: bool,

    /// When `true` and `ocr_text_only` is `false`, append the OCR text after
    /// the image placeholder in the rendered output.
    #[serde(default)]
    pub append_ocr_text: bool,

    /// Target format for re-encoding extracted images.
    ///
    /// When set to anything other than `Native`, each extracted image is
    /// re-encoded to the requested format before being returned. This lets
    /// callers receive uniform output without duplicating encode logic
    /// downstream.
    ///
    /// Defaults to `Native` — no re-encode pass is performed and
    /// `ExtractedImage.format` reflects the source extractor's output.
    #[serde(default)]
    pub output_format: ImageOutputFormat,

    /// SVG-specific knobs for the image-encode pipeline.
    ///
    /// Controls sanitization and rasterization DPI when the source or output
    /// format is SVG.  Only available when the `svg` feature is active.
    #[cfg(feature = "svg")]
    #[serde(default)]
    pub svg: SvgOptions,

    /// When `true`, populate `ExtractedImage::data_base64` with a Base64-encoded
    /// copy of the raw image bytes.
    ///
    /// Useful for JSON-only clients that cannot efficiently parse the default
    /// integer-array serialization of `data`. Defaults to `false`; enabling it
    /// doubles the in-memory image representation for the duration of the response.
    #[serde(default)]
    pub include_data_base64: bool,
}

/// Token reduction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenReductionOptions {
    /// Reduction mode: "off", "light", "moderate", "aggressive", "maximum"
    #[serde(default = "default_reduction_mode")]
    pub mode: String,

    /// Preserve important words (capitalized, technical terms)
    #[serde(default = "default_true")]
    pub preserve_important_words: bool,
}

/// Language detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageDetectionConfig {
    /// Enable language detection
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum confidence threshold (0.0-1.0)
    #[serde(default = "default_confidence")]
    pub min_confidence: f64,

    /// Detect multiple languages in the document
    #[serde(default)]
    pub detect_multiple: bool,
}

impl Default for ImageExtractionConfig {
    fn default() -> Self {
        Self {
            extract_images: true,
            target_dpi: 300,
            max_image_dimension: 4096,
            inject_placeholders: true,
            auto_adjust_dpi: true,
            min_dpi: 72,
            max_dpi: 600,
            max_images_per_page: None,
            classify: false,
            include_page_rasters: false,
            run_ocr_on_images: true,
            ocr_text_only: false,
            append_ocr_text: false,
            output_format: ImageOutputFormat::Native,
            #[cfg(feature = "svg")]
            svg: SvgOptions::default(),
            include_data_base64: false,
        }
    }
}

impl Default for TokenReductionOptions {
    fn default() -> Self {
        Self {
            mode: default_reduction_mode(),
            preserve_important_words: true,
        }
    }
}

impl Default for LanguageDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_confidence: 0.8,
            detect_multiple: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_target_dpi() -> i32 {
    300
}

fn default_max_dimension() -> i32 {
    4096
}

fn default_min_dpi() -> i32 {
    72
}

fn default_max_dpi() -> i32 {
    600
}

fn default_reduction_mode() -> String {
    "off".to_string()
}

fn default_confidence() -> f64 {
    0.8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_extraction_config_default_booleans() {
        let cfg = ImageExtractionConfig::default();
        assert!(cfg.extract_images, "extract_images must default to true");
        assert!(cfg.inject_placeholders, "inject_placeholders must default to true");
        assert!(cfg.auto_adjust_dpi, "auto_adjust_dpi must default to true");
        assert!(!cfg.classify, "classify must default to false (#1116)");
        assert_eq!(cfg.target_dpi, 300);
        assert_eq!(cfg.max_image_dimension, 4096);
        assert_eq!(cfg.min_dpi, 72);
        assert_eq!(cfg.max_dpi, 600);
    }

    #[test]
    fn test_image_extraction_config_defaults() {
        let cfg = ImageExtractionConfig::default();
        assert!(cfg.run_ocr_on_images, "run_ocr_on_images must default to true");
    }

    #[test]
    fn test_image_extraction_config_explicit_false_disables_placeholders() {
        let cfg = ImageExtractionConfig {
            inject_placeholders: false,
            ..ImageExtractionConfig::default()
        };
        assert!(!cfg.inject_placeholders);
        assert!(cfg.extract_images);
    }

    #[test]
    fn test_image_extraction_config_explicit_false_disables_classify() {
        let cfg = ImageExtractionConfig {
            classify: false,
            ..ImageExtractionConfig::default()
        };
        assert!(!cfg.classify);
        assert!(cfg.extract_images);
    }

    #[test]
    fn test_image_extraction_config_absent_json_fields_get_canonical_defaults() {
        let json = r#"{"extract_images": true}"#;
        let cfg: ImageExtractionConfig = serde_json::from_str(json).unwrap();
        assert!(
            cfg.inject_placeholders,
            "absent inject_placeholders must deserialize to true"
        );
        assert!(cfg.auto_adjust_dpi, "absent auto_adjust_dpi must deserialize to true");
        assert_eq!(cfg.target_dpi, 300);
    }

    #[test]
    fn test_max_images_per_page_defaults_none() {
        let config = ImageExtractionConfig::default();
        assert_eq!(config.max_images_per_page, None);
    }

    #[test]
    fn test_max_images_per_page_serializes_as_null_when_none() {
        let config = ImageExtractionConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"max_images_per_page\":null"));
    }

    #[test]
    fn test_max_images_per_page_roundtrips_via_json() {
        let config = ImageExtractionConfig {
            max_images_per_page: Some(50),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ImageExtractionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_images_per_page, Some(50));
    }

    /// Regression test for issue #766: missing field in JSON must not break
    /// deserialization (backwards-compat — existing configs without this key
    /// must still deserialize cleanly).
    #[test]
    fn test_max_images_per_page_absent_in_json_deserializes_as_none() {
        let json = r#"{"extract_images":true,"target_dpi":300,"max_image_dimension":4096,
                       "inject_placeholders":true,"auto_adjust_dpi":true,
                       "min_dpi":72,"max_dpi":600}"#;
        let config: ImageExtractionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_images_per_page, None);
    }

    #[test]
    fn test_include_page_rasters_defaults_false() {
        let config = ImageExtractionConfig::default();
        assert!(
            !config.include_page_rasters,
            "include_page_rasters must default to false"
        );
    }

    #[test]
    fn test_include_page_rasters_absent_in_json_deserializes_as_false() {
        let json = r#"{"extract_images":true,"target_dpi":300,"max_image_dimension":4096,
                       "inject_placeholders":true,"auto_adjust_dpi":true,
                       "min_dpi":72,"max_dpi":600}"#;
        let config: ImageExtractionConfig = serde_json::from_str(json).unwrap();
        assert!(
            !config.include_page_rasters,
            "absent include_page_rasters must deserialize to false (backward compat)"
        );
    }

    #[test]
    fn test_include_page_rasters_roundtrips_via_json() {
        let config = ImageExtractionConfig {
            include_page_rasters: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ImageExtractionConfig = serde_json::from_str(&json).unwrap();
        assert!(back.include_page_rasters);
    }

    #[test]
    fn test_image_output_format_default_is_native() {
        assert_eq!(ImageOutputFormat::default(), ImageOutputFormat::Native);
    }

    #[test]
    fn test_image_extraction_config_default_output_format_is_native() {
        let cfg = ImageExtractionConfig::default();
        assert_eq!(cfg.output_format, ImageOutputFormat::Native);
    }

    #[test]
    fn test_image_extraction_config_empty_json_gives_native_output_format() {
        let cfg: ImageExtractionConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.output_format, ImageOutputFormat::Native);
    }

    #[test]
    fn test_output_format_native_roundtrips_via_json() {
        let fmt = ImageOutputFormat::Native;
        let json = serde_json::to_string(&fmt).unwrap();
        assert_eq!(json, r#"{"type":"native"}"#);
        let back: ImageOutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImageOutputFormat::Native);
    }

    #[test]
    fn test_output_format_png_roundtrips_via_json() {
        let fmt = ImageOutputFormat::Png;
        let json = serde_json::to_string(&fmt).unwrap();
        assert_eq!(json, r#"{"type":"png"}"#);
        let back: ImageOutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImageOutputFormat::Png);
    }

    #[test]
    fn test_output_format_jpeg_roundtrips_via_json() {
        let fmt = ImageOutputFormat::Jpeg { quality: 90 };
        let json = serde_json::to_string(&fmt).unwrap();
        let back: ImageOutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImageOutputFormat::Jpeg { quality: 90 });
    }

    #[test]
    fn test_output_format_jpeg_default_quality_applied_when_field_absent() {
        let json = r#"{"type":"jpeg"}"#;
        let fmt: ImageOutputFormat = serde_json::from_str(json).unwrap();
        assert_eq!(fmt, ImageOutputFormat::Jpeg { quality: 85 });
    }

    #[test]
    fn test_output_format_webp_roundtrips_via_json() {
        let fmt = ImageOutputFormat::Webp { quality: 75 };
        let json = serde_json::to_string(&fmt).unwrap();
        let back: ImageOutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImageOutputFormat::Webp { quality: 75 });
    }

    #[test]
    fn test_output_format_webp_default_quality_applied_when_field_absent() {
        let json = r#"{"type":"webp"}"#;
        let fmt: ImageOutputFormat = serde_json::from_str(json).unwrap();
        assert_eq!(fmt, ImageOutputFormat::Webp { quality: 80 });
    }

    #[test]
    fn test_output_format_webp_wire_value_is_lowercase_webp() {
        let fmt = ImageOutputFormat::Webp { quality: 80 };
        let json = serde_json::to_string(&fmt).unwrap();
        assert_eq!(json, r#"{"type":"webp","quality":80}"#);
    }

    #[cfg(feature = "heic")]
    #[test]
    fn test_output_format_heif_roundtrips_via_json() {
        let fmt = ImageOutputFormat::Heif { quality: 70 };
        let json = serde_json::to_string(&fmt).unwrap();
        let back: ImageOutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImageOutputFormat::Heif { quality: 70 });
    }

    #[cfg(feature = "heic")]
    #[test]
    fn test_output_format_heif_default_quality_applied_when_field_absent() {
        let json = r#"{"type":"heif"}"#;
        let fmt: ImageOutputFormat = serde_json::from_str(json).unwrap();
        assert_eq!(fmt, ImageOutputFormat::Heif { quality: 80 });
    }

    #[test]
    fn test_output_format_in_image_extraction_config_roundtrips_via_json() {
        let config = ImageExtractionConfig {
            output_format: ImageOutputFormat::Jpeg { quality: 92 },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ImageExtractionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.output_format, ImageOutputFormat::Jpeg { quality: 92 });
    }
}
