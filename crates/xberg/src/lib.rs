//! Xberg - High-Performance Document Intelligence Library
//!
//! Xberg is a Rust-first document extraction library with language-agnostic plugin support.
//! It provides fast, accurate extraction from PDFs, images, Office documents, emails, and more.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use xberg::{extract, ExtractInput, ExtractionConfig};
//!
//! # async fn run() -> xberg::Result<()> {
//! let config = ExtractionConfig::default();
//! let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
//! println!("Extracted: {}", output.results[0].content);
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! - **Core Module** (`core`): Main extraction orchestration, MIME detection, config loading
//! - **Plugin System**: Language-agnostic plugin architecture
//! - **Extractors**: Format-specific extraction (PDF, images, Office docs, email, etc.)
//! - **OCR**: Multiple OCR backend support (Tesseract, PaddleOCR, VLM)
//!
//! # Features
//!
//! - Fast parallel processing with async/await
//! - Priority-based extractor selection
//! - Comprehensive MIME type detection (115 file extensions)
//! - Configurable caching and quality processing
//! - Cross-language plugin support (Python, Node.js planned)

#![deny(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(test, allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro))]
#![deny(unsafe_code)]

#[cfg(all(
    feature = "sceptre-ocr-ort",
    any(target_arch = "wasm32", target_os = "android", target_os = "ios")
))]
compile_error!("`sceptre-ocr-ort` supports desktop and server targets only; use `sceptre-ocr-tract` on mobile");

#[cfg(all(
    feature = "paddle-ocr-ort",
    any(target_arch = "wasm32", target_os = "android", target_os = "ios")
))]
compile_error!("`paddle-ocr-ort` supports desktop and server targets only; use `paddle-ocr-tract` on mobile");

pub mod cache;
pub(crate) mod cache_dir;
pub mod cancellation;
pub mod core;
pub mod engine;
pub mod error;
/// Format-specific document extraction implementations and office metadata types.
pub mod extraction;
pub mod extractors;
#[cfg(all(
    feature = "layout-detection",
    any(feature = "pdf", feature = "ocr", feature = "ocr-wasm")
))]
pub mod model_cache;
pub mod plugins;
pub mod rendering;
pub mod telemetry;
/// Text post-processing: NER, summarisation, redaction, token reduction, and translation.
pub mod text;
pub mod types;
pub mod utils;

#[cfg(any(feature = "ocr", feature = "pdf", paddle_ocr))]
pub mod table_core;

#[cfg(feature = "tower-service")]
pub mod service;

#[cfg(feature = "api")]
pub mod api;

#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(feature = "chunking")]
pub mod chunking;

#[cfg(feature = "diff")]
pub mod diff;

#[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
pub mod llm;

#[cfg(feature = "embedding-presets")]
pub mod embeddings;

#[cfg(any(feature = "reranker-presets", feature = "reranker"))]
pub mod reranking;

/// Shared ONNX Runtime model-loading helpers (download, tokenizer, session).
#[cfg(feature = "onnx-runtime")]
pub(crate) mod onnx;

/// Sparse (SPLADE) learned embeddings for hybrid dense+sparse retrieval.
#[cfg(any(feature = "sparse-embedding-presets", feature = "sparse-embeddings"))]
pub mod sparse_embeddings;

/// ColBERT late-interaction (multi-vector) embeddings for MaxSim retrieval.
#[cfg(any(feature = "late-interaction-presets", feature = "late-interaction"))]
pub mod late_interaction;

#[cfg(feature = "ocr")]
/// Image preprocessing and DPI utilities for OCR pipelines.
pub mod image;

#[cfg(feature = "language-detection")]
pub mod language_detection;

#[cfg(feature = "stopwords")]
pub mod stopwords;

#[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
pub mod keywords;

#[cfg(feature = "enrichment")]
pub mod enrichment;

#[cfg(feature = "heuristics")]
pub mod heuristics;

#[cfg(feature = "heuristics")]
pub use heuristics::{
    BoundaryReason, ChunkInfo, ChunkPlan, ChunkingDecision, ChunkingReason, ConfidenceSignals, ConfidenceWeights,
    DocumentBoundary, DocumentMetadata, ExtractionConfidence, HeuristicsConfig, HeuristicsError, MultidocInput,
    MultidocThresholds, NoChunkingReason, PageRange, PageSignals, SchemaCompliance, StructuredCallMode,
    StructuredInput, StructuredThresholds, UserChunkConfig, analyze_document, analyze_with_user_chunks,
    boundaries_from_extraction_result, calculate_chunk_plan, calculate_plan_from_overrides, check_format_limits,
    choose_call_mode, detect_boundaries, score_confidence,
};

#[cfg(feature = "presets")]
pub mod presets;

#[cfg(any(feature = "ocr", feature = "ocr-wasm"))]
pub mod ocr;

/// Canonical OCR metadata key names, shared across every OCR-producing and
/// OCR-consuming feature domain. Deliberately ungated — see the module docs.
pub(crate) mod ocr_metadata_keys;

pub mod doctor;

#[cfg(any(
    paddle_ocr,
    feature = "embeddings",
    feature = "reranker",
    feature = "onnx-runtime",
    feature = "layout-detection",
    feature = "auto-rotate",
    feature = "transcription"
))]
pub mod ort_discovery;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod model_download;

/// Engine-neutral inference seam (issue #1275): backend/session traits over ONNX
/// Runtime on native builds and the pure-Rust `tract` engine on no-ORT targets
/// (Android x86_64; WASM once embedded-weight loading lands). `auto_rotate` covers
/// both the ORT `auto-rotate` and the tract `auto-rotate-tract` variants; `layout_detection`
/// covers both the ORT `layout-detection` and the tract `layout-tract` variants.
#[cfg(any(layout_detection, auto_rotate))]
pub(crate) mod inference;

#[cfg(any(paddle_ocr, feature = "paddle-ocr-types"))]
pub mod paddle_ocr;

#[cfg(all(sceptre_ocr, not(target_arch = "wasm32")))]
pub mod sceptre_ocr;

#[cfg(any(sceptre_ocr, feature = "sceptre-wasm"))]
mod sceptre_languages;

#[cfg(feature = "sceptre-wasm")]
pub mod sceptre_wasm;

#[cfg(feature = "candle-ocr")]
pub mod candle_ocr;

#[cfg(feature = "auto-rotate-types")]
pub mod doc_orientation;

#[cfg(feature = "layout-types")]
pub mod layout;

/// LaTeX recognition for rasterized formula regions (RapidLaTeXOCR ONNX).
#[cfg(feature = "formula-recognition")]
pub mod formula_recognition;

#[cfg(feature = "pdf")]
pub mod pdf;

#[cfg(feature = "transcription")]
pub mod transcription;

#[cfg(feature = "captioning")]
pub mod captioning;

// NOTE: `CancellationToken` is intentionally NOT re-exported here.
pub use error::{Result, XbergError};
pub use types::*;

// root (`#[frb(mirror(CoreProperties))]` → `xberg::CoreProperties`).
#[cfg(feature = "office")]
pub use extraction::office_metadata::{CoreProperties, DocxAppProperties};

#[cfg(feature = "url-ingestion")]
pub use core::extract::map_url;
pub use core::extract::{extract, extract_batch};
#[cfg(feature = "pdf")]
pub use core::split::{SplitConfig, SplitSegment, SplitStrategy, split_and_extract};

pub use core::config::{
    AccelerationConfig, BedrockConfig, BreadcrumbTarget, CallMode, CaptioningConfig, ChunkClassificationConfig,
    ChunkClassificationDefinition, ChunkSizing, ChunkerType, ChunkingConfig, ContentFilterConfig,
    CredentialProviderConfig, CsvConfig, EmailConfig, EmbeddingConfig, EmbeddingModelType, ExecutionProviderType,
    ExtractInput, ExtractInputKind, ExtractionConfig, ExtractionErrorItem, ExtractionResult, ExtractionSummary,
    FileExtractionConfig, ImageExtractionConfig, JupyterCellRendering, LanguageDetectionConfig, LlmConfig, MergeMode,
    NerBackendKind, NerConfig, OcrConfig, OutputFormat, PageClassificationConfig, PageConfig, PostProcessorConfig,
    RedactionConfig, RedactionPattern, RedactionTerm, RerankerConfig, RerankerHead, RerankerModelType,
    StructuredExtractionConfig, SummarizationConfig, TableChunkingMode, TokenReductionOptions, TranslationConfig,
    UrlExtractionConfig, UrlExtractionMode,
};
pub use core::config::{
    LateInteractionConfig, LateInteractionModelType, SparseEmbeddingConfig, SparseEmbeddingModelType,
};
#[cfg(feature = "transcription-types")]
pub use core::config::{TranscriptionConfig, WhisperModel};
#[cfg(any(feature = "url-ingestion", feature = "url-config-types"))]
pub use crawlberg::{
    AssetCategory, AuthConfig, BrowserBackend, BrowserConfig, BrowserMode, BrowserWait, ContentConfig, CrawlConfig,
    ProxyConfig, SsrfPolicy,
};
#[cfg(feature = "url-ingestion")]
pub use crawlberg::{MapResult, SitemapUrl};
pub use extractors::security::SecurityLimits;

#[cfg(feature = "presets")]
pub use presets::{
    LoadError, MetaSchema, Preset, PresetCategory, PresetSample, PresetSummary, Registry, ResolveError, ResolvedPreset,
    resolve,
};

#[cfg(feature = "quality")]
pub use text::{ReductionLevel, TokenReductionConfig};

#[cfg(all(
    feature = "ner-llm",
    not(target_arch = "wasm32"),
    not(all(target_os = "android", target_arch = "x86_64"))
))]
#[cfg_attr(alef, alef(skip))]
pub use text::ner::llm::LlmBackend;

#[cfg(feature = "ner-llm")]
pub use text::ner::NerBackend;

#[cfg(any(not(feature = "ner-llm"), all(target_os = "android", target_arch = "x86_64")))]
#[derive(Clone, Debug)]
#[cfg_attr(alef, alef(skip))]
pub struct LlmBackend {
    _config: LlmConfig,
}

#[cfg(any(not(feature = "ner-llm"), all(target_os = "android", target_arch = "x86_64")))]
impl LlmBackend {
    pub fn new(config: LlmConfig) -> Self {
        Self { _config: config }
    }

    pub async fn detect(&self, _text: &str, _categories: &[crate::EntityCategory]) -> Result<Vec<crate::Entity>> {
        Err(crate::XbergError::Other(
            "ner-llm feature not available on this target".into(),
        ))
    }

    pub async fn detect_with_custom(
        &self,
        _text: &str,
        _categories: &[crate::EntityCategory],
        _custom_labels: &[String],
    ) -> Result<Vec<crate::Entity>> {
        Err(crate::XbergError::Other(
            "ner-llm feature not available on this target".into(),
        ))
    }
}

#[cfg(feature = "ner-onnx")]
pub use text::ner::gline::GlineBackend;

#[cfg(not(feature = "ner-onnx"))]
#[derive(Clone, Debug)]
pub struct GlineBackend {
    pub repo_id: String,
    pub model_path: std::path::PathBuf,
    pub tokenizer_path: std::path::PathBuf,
}

#[cfg(not(feature = "ner-onnx"))]
impl GlineBackend {
    pub fn new(_repo_id: Option<&str>) -> Result<Self> {
        Err(crate::XbergError::Other(
            "ner-onnx feature not available on this target".into(),
        ))
    }

    pub async fn detect(&self, _text: &str, _categories: &[crate::EntityCategory]) -> Result<Vec<crate::Entity>> {
        Err(crate::XbergError::Other(
            "ner-onnx feature not available on this target".into(),
        ))
    }

    pub async fn detect_with_custom(
        &self,
        _text: &str,
        _categories: &[crate::EntityCategory],
        _custom_labels: &[String],
    ) -> Result<Vec<crate::Entity>> {
        Err(crate::XbergError::Other(
            "ner-onnx feature not available on this target".into(),
        ))
    }
}

#[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
pub use llm::region_extractor::RegionKind;

#[cfg(not(all(feature = "liter-llm", not(target_arch = "wasm32"))))]
/// Per-region VLM extraction type stub.
///
/// Identifies the semantic kind of a document region for VLM-based extraction.
/// This stub is emitted on targets where the `liter-llm` feature is unavailable
/// (WASM, Android x86_64 emulator) so that alef-generated bindings compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RegionKind {
    /// A figure or illustration region.
    Figure,
    /// A data-dense table region.
    DenseTable,
    /// A region with complex multi-column or mixed layout.
    ComplexLayout,
    /// A figure or table caption.
    Caption,
}

#[cfg(not(all(feature = "liter-llm", not(target_arch = "wasm32"))))]
impl RegionKind {
    /// Returns an empty default prompt string for this stub implementation.
    pub fn default_prompt(self) -> &'static str {
        ""
    }
}

#[cfg(feature = "ner")]
#[cfg_attr(alef, alef(skip))]
pub use text::ner::detect_entities;

#[cfg(feature = "classification")]
#[cfg_attr(alef, alef(skip))]
pub use text::classification::classify_document;

#[cfg(feature = "redaction")]
pub use text::redaction::strategy::TokenCounter;

#[cfg(feature = "api-types")]
pub use core::server_config::ServerConfig;

#[cfg(feature = "pdf")]
pub use core::config::{HierarchyConfig, PdfConfig};

#[cfg(feature = "html")]
pub use core::config::{HtmlOutputConfig, HtmlTheme};
// `ExtractionConfig::html_options` and `FileConfig::html_options` are public fields of this
// external type, so callers already have to name it; without this re-export they must take a
// direct `html-to-markdown-rs` dependency and keep its version in lockstep with ours. It is
// also what the generated bindings resolve against — the wasm serde mirror emits
// `xberg::ConversionOptions`, which is the same path every sibling config type uses. ~keep
#[cfg(feature = "html")]
pub use html_to_markdown_rs::ConversionOptions;
#[cfg(feature = "html")]
pub use rendering::StyledHtmlRenderer;

#[cfg(feature = "paddle-ocr-types")]
pub use paddle_ocr::{ModelPaths, PaddleInferenceBackend, PaddleLanguage, PaddleOcrConfig};

#[cfg(paddle_ocr)]
pub use paddle_ocr::{ModelCacheStats, ModelManager, ModelManifestEntry, PaddleOcrBackend};

pub use cache::CacheStats;

#[cfg(feature = "layout-types")]
pub use core::config::{LayoutDetectionConfig, LayoutStrategy, TableModel};

#[cfg(feature = "layout-types")]
pub use layout::types::{BBox, DetectionResult, LayoutClass, LayoutDetection};

#[cfg(feature = "layout-types")]
pub use layout::types::RecognizedTable;
#[cfg(any(feature = "ocr", feature = "ocr-wasm"))]
pub use ocr::types::PSMMode;

pub use core::config::{OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds, OcrStrategy, VlmFallbackPolicy};

#[cfg(feature = "auto-rotate-types")]
pub use doc_orientation::OrientationResult;

#[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
pub use keywords::{Keyword, KeywordAlgorithm, KeywordConfig};

#[cfg(feature = "keywords-rake")]
pub use keywords::RakeParams;

#[cfg(feature = "keywords-yake")]
pub use keywords::YakeParams;

#[cfg(feature = "markdown-footnotes")]
pub use text::markdown_footnotes::{
    Citation, FootnoteAnchor, FootnoteConfig, FootnoteDefinition, find_footnote_anchors, find_inference_markers,
    find_unmarked_claims, parse_citations, parse_footnote_definitions, verify_excerpt,
};

#[cfg(feature = "diff")]
pub use diff::{DiffHunk, DiffOptions, EmbeddedChanges, EmbeddedDiff, ExtractionDiff, TableDiff, compare};

#[cfg(feature = "tree-sitter")]
pub use core::config::{CodeContentMode, TreeSitterConfig, TreeSitterProcessConfig};
#[cfg(feature = "tree-sitter")]
pub use tree_sitter_language_pack::{
    CommentKind, DiagnosticSeverity, ExportKind, FileMetrics, ProcessConfig, StructureKind,
};

pub use core::mime::{SupportedFormat, detect_mime_type_from_bytes, get_extensions_for_mime, list_supported_formats};

/// Detect the MIME type of a file at the given path.
///
/// Uses the file extension and optionally the file content to determine the MIME type.
/// Set `check_exists` to `true` to verify the file exists before detection.
pub fn detect_mime_type(path: String, check_exists: bool) -> crate::Result<String> {
    core::mime::detect_mime_type(path, check_exists)
}

#[cfg(feature = "pdf")]
pub use pdf::render::{pdf_page_count, render_pdf_page_to_png};

pub use doctor::{DoctorCheck, DoctorReport, ProbeStatus, doctor};

#[cfg_attr(alef, alef(skip))]
pub use plugins::{
    clear_document_extractors, clear_embedding_backends, clear_ocr_backends, clear_post_processors, clear_renderers,
    clear_reranker_backends, clear_tokenizer_backends, clear_validators, list_document_extractors,
    list_embedding_backends, list_ocr_backends, list_post_processors, list_renderers, list_reranker_backends,
    list_tokenizer_backends, list_validators, register_document_extractor, register_embedding_backend,
    register_ocr_backend, register_post_processor, register_renderer, register_reranker_backend,
    register_tokenizer_backend, register_validator, unregister_document_extractor, unregister_embedding_backend,
    unregister_ocr_backend, unregister_post_processor, unregister_renderer, unregister_reranker_backend,
    unregister_tokenizer_backend, unregister_validator,
};

#[cfg_attr(alef, alef(skip))]
pub use plugins::{
    DocumentExtractor, EmbeddingBackend, OcrBackend, OcrBackendType, PostProcessor, ProcessingStage, Renderer,
    RerankerBackend, TokenizerBackend, Validator,
};

#[cfg(feature = "embedding-presets")]
pub use embeddings::EmbeddingPreset;

/// Embed a list of texts using the configured embedding model.
///
/// Returns a 2D vector where each inner vector is the embedding for the corresponding text.
#[cfg(any(feature = "embeddings", feature = "static-embeddings"))]
#[cfg_attr(alef, alef(skip))]
pub fn embed_texts(texts: Vec<String>, config: &core::config::EmbeddingConfig) -> crate::Result<Vec<Vec<f32>>> {
    embeddings::embed_texts(&texts, config)
}

/// Stub for builds without the `embeddings` or `static-embeddings` feature —
/// keeps the symbol available so language bindings that mirror the public API
/// compile; the runtime call returns an unsupported error.
#[cfg(all(
    feature = "embedding-presets",
    not(feature = "embeddings"),
    not(feature = "static-embeddings")
))]
#[cfg_attr(alef, alef(skip))]
pub fn embed_texts(_texts: Vec<String>, _config: &core::config::EmbeddingConfig) -> crate::Result<Vec<Vec<f32>>> {
    Err(XbergError::validation(
        "embed_texts requires the `embeddings` (ONNX Runtime) or `static-embeddings` (pure-Rust) feature; \
         neither is enabled on this build",
    ))
}

#[cfg(all(
    feature = "tokio-runtime",
    any(feature = "embeddings", feature = "static-embeddings")
))]
#[cfg_attr(alef, alef(skip))]
pub use embeddings::embed_texts_async;

#[cfg(all(
    feature = "embedding-presets",
    not(feature = "embeddings"),
    not(feature = "static-embeddings"),
    feature = "tokio-runtime"
))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_texts_async(
    _texts: Vec<String>,
    _config: &core::config::EmbeddingConfig,
) -> crate::Result<Vec<Vec<f32>>> {
    Err(XbergError::validation(
        "embed_texts_async requires the `embeddings` (ONNX Runtime) or `static-embeddings` (pure-Rust) feature; \
         neither is enabled on this build",
    ))
}

/// Get an embedding preset by name.
///
/// Returns `None` if no preset with the given name exists. Returns an owned
/// clone so the value is safe to pass across FFI boundaries.
#[cfg(feature = "embedding-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn get_embedding_preset(name: &str) -> Option<embeddings::EmbeddingPreset> {
    embeddings::get_preset(name)
}

/// List the names of all available embedding presets.
///
/// Returns owned `String`s so the values are safe to pass across FFI boundaries.
#[cfg(feature = "embedding-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn list_embedding_presets() -> Vec<String> {
    embeddings::list_presets()
}

/// Query-side instruction prefix for an embedding config, if its preset defines
/// one (asymmetric retrieval models such as Arctic-Embed). The RAG query path
/// prepends this to query text; document text is embedded verbatim. Returns
/// `None` for symmetric presets, custom models, and non-preset backends.
#[cfg(feature = "embedding-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn embedding_query_prefix(config: &EmbeddingConfig) -> Option<String> {
    embeddings::embedding_query_prefix(config)
}

/// Stub preset type for builds without the `embedding-presets` feature.
///
/// Field names match the real type so JSON round-trips through
/// `xberg_embedding_preset_from_json` remain schema-compatible. When the
/// feature is absent, `get_embedding_preset` always returns `None`, so the
/// stub is never allocated in practice.
#[cfg(not(feature = "embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingPreset {
    /// Unique preset identifier (e.g. "balanced", "multilingual").
    pub name: String,
    /// Maximum input size in Unicode characters for chunking.
    pub chunk_size: usize,
    /// Overlap in characters between adjacent chunks.
    pub overlap: usize,
    /// HuggingFace repository ID for the model (e.g. "BAAI/bge-small-en-v1.5").
    pub model_repo: String,
    /// Pooling strategy used to aggregate token embeddings (e.g. "mean", "cls").
    pub pooling: String,
    /// ONNX model file name within the repository.
    pub model_file: String,
    /// Number of dimensions in the embedding output vectors.
    pub dimensions: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
}

/// Returns `None` for builds without the `embedding-presets` feature.
#[cfg(not(feature = "embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_embedding_preset(_name: &str) -> Option<EmbeddingPreset> {
    None
}

/// Returns an empty list for builds without the `embedding-presets` feature.
#[cfg(not(feature = "embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_embedding_presets() -> Vec<String> {
    Vec::new()
}

/// Re-export `RerankerPreset` when the `reranker-presets` feature is active.
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
pub use reranking::RerankerPreset;

/// Re-export `RerankedDocument` — needed for stub signatures and result types.
///
/// Since v5.0.0.
#[cfg(any(feature = "reranker-presets", feature = "reranker"))]
pub use reranking::RerankedDocument;

/// Rerank a list of documents by relevance to a query.
///
/// Returns documents sorted descending by score. Applies `top_k` truncation if
/// configured.
///
/// # Errors
///
/// - [`XbergError::Validation`] if `query` is empty or blank.
/// - [`XbergError::MissingDependency`] if ONNX Runtime is not installed (ONNX path).
/// - [`XbergError::Reranking`] if the preset is unknown or model download fails.
///
/// Since v5.0.0.
#[cfg(feature = "reranker")]
#[cfg_attr(alef, alef(skip))]
pub fn rerank(
    query: String,
    documents: Vec<String>,
    config: &core::config::RerankerConfig,
) -> crate::Result<Vec<reranking::RerankedDocument>> {
    reranking::rerank(query, documents, config)
}

/// Stub for builds without the `reranker` feature — keeps the symbol available
/// on no-ORT targets (Android x86_64 emulator, WASM) so language bindings compile.
///
/// Since v5.0.0.
#[cfg(all(feature = "reranker-presets", not(feature = "reranker")))]
#[cfg_attr(alef, alef(skip))]
pub fn rerank(
    _query: String,
    _documents: Vec<String>,
    _config: &core::config::RerankerConfig,
) -> crate::Result<Vec<reranking::RerankedDocument>> {
    Err(XbergError::validation(
        "rerank requires the `reranker` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

#[cfg(all(feature = "reranker", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use reranking::rerank_async;

/// Stub for builds without the `reranker` feature.
///
/// Since v5.0.0.
#[doc(alias = "rerank")]
#[cfg(all(feature = "reranker-presets", not(feature = "reranker"), feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub async fn rerank_async(
    _query: String,
    _documents: Vec<String>,
    _config: &core::config::RerankerConfig,
) -> crate::Result<Vec<reranking::RerankedDocument>> {
    Err(XbergError::validation(
        "rerank_async requires the `reranker` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

/// Get a reranker preset by name.
///
/// Returns `None` if no preset with the given name exists. Returns an owned
/// clone so the value is safe to pass across FFI boundaries.
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn get_reranker_preset(name: &str) -> Option<reranking::RerankerPreset> {
    reranking::get_preset(name)
}

/// List the names of all available reranker presets.
///
/// Returns owned `String`s so the values are safe to pass across FFI boundaries.
///
/// Since v5.0.0.
#[cfg(feature = "reranker-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn list_reranker_presets() -> Vec<String> {
    reranking::list_presets()
}

/// Stub preset type for builds without the `reranker-presets` feature.
///
/// Field names match the real type so JSON round-trips remain schema-compatible.
/// When the feature is absent, `get_reranker_preset` always returns `None`, so
/// the stub is never allocated in practice.
///
/// Since v5.0.0.
#[cfg(not(feature = "reranker-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RerankerPreset {
    /// Unique preset identifier (e.g. "balanced", "multilingual").
    pub name: String,
    /// HuggingFace repository ID for the model.
    pub model_repo: String,
    /// ONNX model file name within the repository.
    pub model_file: String,
    /// Maximum token sequence length the model supports.
    pub max_length: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
}

/// Stub result document type for builds without `reranker-presets`.
///
/// Since v5.0.0.
#[cfg(not(feature = "reranker-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RerankedDocument {
    /// Position of this document in the original input slice.
    pub index: usize,
    /// Relevance score in `[0, 1]`.
    pub score: f32,
    /// The document text.
    pub document: String,
}

/// Returns `None` for builds without the `reranker-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "reranker-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_reranker_preset(_name: &str) -> Option<RerankerPreset> {
    None
}

/// Returns an empty list for builds without the `reranker-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "reranker-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_reranker_presets() -> Vec<String> {
    Vec::new()
}

/// Re-export the sparse-embedding result and preset types when the presets
/// feature is active.
///
/// Since v5.0.0.
#[cfg(feature = "sparse-embedding-presets")]
pub use sparse_embeddings::{SparseEmbedding, SparseEmbeddingPreset};

/// Generate sparse (SPLADE) embeddings for a list of texts.
///
/// Returns one [`SparseEmbedding`] per input text, in order.
///
/// Since v5.0.0.
#[cfg(feature = "sparse-embeddings")]
#[cfg_attr(alef, alef(skip))]
pub fn embed_sparse(
    texts: Vec<String>,
    config: &core::config::SparseEmbeddingConfig,
) -> crate::Result<Vec<SparseEmbedding>> {
    sparse_embeddings::embed_sparse(&texts, config)
}

/// Stub for builds without the `sparse-embeddings` feature — keeps the symbol
/// available on no-ORT targets so language bindings compile; the runtime call
/// returns an unsupported error.
///
/// Since v5.0.0.
#[cfg(all(feature = "sparse-embedding-presets", not(feature = "sparse-embeddings")))]
#[cfg_attr(alef, alef(skip))]
pub fn embed_sparse(
    _texts: Vec<String>,
    _config: &core::config::SparseEmbeddingConfig,
) -> crate::Result<Vec<SparseEmbedding>> {
    Err(XbergError::validation(
        "embed_sparse requires the `sparse-embeddings` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

#[cfg(all(feature = "sparse-embeddings", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use sparse_embeddings::embed_sparse_async;

/// Stub for builds without the `sparse-embeddings` feature.
///
/// Since v5.0.0.
#[cfg(all(
    feature = "sparse-embedding-presets",
    not(feature = "sparse-embeddings"),
    feature = "tokio-runtime"
))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_sparse_async(
    _texts: Vec<String>,
    _config: &core::config::SparseEmbeddingConfig,
) -> crate::Result<Vec<SparseEmbedding>> {
    Err(XbergError::validation(
        "embed_sparse_async requires the `sparse-embeddings` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

/// Get a sparse-embedding preset by name.
///
/// Since v5.0.0.
#[cfg(feature = "sparse-embedding-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn get_sparse_embedding_preset(name: &str) -> Option<sparse_embeddings::SparseEmbeddingPreset> {
    sparse_embeddings::get_preset(name)
}

/// List the names of all available sparse-embedding presets.
///
/// Since v5.0.0.
#[cfg(feature = "sparse-embedding-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn list_sparse_embedding_presets() -> Vec<String> {
    sparse_embeddings::list_presets()
}

/// Stub result type for builds without the `sparse-embedding-presets` feature.
///
/// Field names match the real type so JSON round-trips remain schema-compatible.
///
/// Since v5.0.0.
#[cfg(not(feature = "sparse-embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct SparseEmbedding {
    /// Vocabulary token ids with non-zero weight, ascending.
    pub indices: Vec<u32>,
    /// Weights parallel to `indices`.
    pub values: Vec<f32>,
}

/// Stub preset type for builds without the `sparse-embedding-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "sparse-embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SparseEmbeddingPreset {
    /// Unique preset identifier (e.g. "splade").
    pub name: String,
    /// HuggingFace repository ID for the model.
    pub model_repo: String,
    /// ONNX model file name within the repository.
    pub model_file: String,
    /// Sibling files that must be downloaded alongside `model_file`.
    pub additional_files: Vec<String>,
    /// Maximum token sequence length the model supports.
    pub max_length: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
}

/// Returns `None` for builds without the `sparse-embedding-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "sparse-embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_sparse_embedding_preset(_name: &str) -> Option<SparseEmbeddingPreset> {
    None
}

/// Returns an empty list for builds without the `sparse-embedding-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "sparse-embedding-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_sparse_embedding_presets() -> Vec<String> {
    Vec::new()
}

/// Re-export the multi-vector result/preset types and the pure-CPU MaxSim
/// primitives when the presets feature is active.
///
/// Since v5.0.0.
#[cfg(feature = "late-interaction-presets")]
pub use late_interaction::{
    LateInteractionMatch, LateInteractionPreset, MultiVectorEmbedding, max_sim_rank, max_sim_score,
};

/// Generate ColBERT multi-vector embeddings for a list of texts.
///
/// `is_query` selects `[Q]`/`[D]` marker insertion and, when `true`, query
/// augmentation padding.
///
/// Since v5.0.0.
#[cfg(feature = "late-interaction")]
#[cfg_attr(alef, alef(skip))]
pub fn embed_multi_vector(
    texts: Vec<String>,
    config: &core::config::LateInteractionConfig,
    is_query: bool,
) -> crate::Result<Vec<MultiVectorEmbedding>> {
    late_interaction::embed_multi_vector(&texts, config, is_query)
}

/// Stub for builds without the `late-interaction` feature — keeps the symbol
/// available on no-ORT targets so language bindings compile.
///
/// Since v5.0.0.
#[cfg(all(feature = "late-interaction-presets", not(feature = "late-interaction")))]
#[cfg_attr(alef, alef(skip))]
pub fn embed_multi_vector(
    _texts: Vec<String>,
    _config: &core::config::LateInteractionConfig,
    _is_query: bool,
) -> crate::Result<Vec<MultiVectorEmbedding>> {
    Err(XbergError::validation(
        "embed_multi_vector requires the `late-interaction` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

#[cfg(all(feature = "late-interaction", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use late_interaction::embed_multi_vector_async;

/// Stub for builds without the `late-interaction` feature.
///
/// Since v5.0.0.
#[cfg(all(
    feature = "late-interaction-presets",
    not(feature = "late-interaction"),
    feature = "tokio-runtime"
))]
#[cfg_attr(alef, alef(skip))]
pub async fn embed_multi_vector_async(
    _texts: Vec<String>,
    _config: &core::config::LateInteractionConfig,
    _is_query: bool,
) -> crate::Result<Vec<MultiVectorEmbedding>> {
    Err(XbergError::validation(
        "embed_multi_vector_async requires the `late-interaction` feature, which depends on ONNX Runtime; \
         not available on this target (Android x86_64 emulator or WASM)",
    ))
}

/// Get a late-interaction preset by name.
///
/// Since v5.0.0.
#[cfg(feature = "late-interaction-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn get_late_interaction_preset(name: &str) -> Option<late_interaction::LateInteractionPreset> {
    late_interaction::get_preset(name)
}

/// List the names of all available late-interaction presets.
///
/// Since v5.0.0.
#[cfg(feature = "late-interaction-presets")]
#[cfg_attr(alef, alef(skip))]
pub fn list_late_interaction_presets() -> Vec<String> {
    late_interaction::list_presets()
}

/// Stub multi-vector result type for builds without the `late-interaction-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "late-interaction-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct MultiVectorEmbedding {
    /// Number of attention-live token rows.
    pub num_tokens: u32,
    /// Dimensionality of each per-token vector.
    pub dim: u32,
    /// Flat row-major buffer, length `num_tokens * dim`.
    pub data: Vec<f32>,
}

/// Stub match type for builds without the `late-interaction-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "late-interaction-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LateInteractionMatch {
    /// Position of this document in the original input slice.
    pub index: usize,
    /// MaxSim relevance score.
    pub score: f32,
}

/// Stub preset type for builds without the `late-interaction-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "late-interaction-presets"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LateInteractionPreset {
    /// Unique preset identifier (e.g. "colbert").
    pub name: String,
    /// HuggingFace repository ID for the model.
    pub model_repo: String,
    /// ONNX model file name within the repository.
    pub model_file: String,
    /// Sibling files that must be downloaded alongside `model_file`.
    pub additional_files: Vec<String>,
    /// Maximum document token sequence length.
    pub max_length: usize,
    /// Fixed padded query length (ColBERT query augmentation).
    pub query_max_length: usize,
    /// Per-token embedding dimensionality.
    pub dim: usize,
    /// Human-readable description of the preset's intended use case.
    pub description: String,
}

/// Returns `None` for builds without the `late-interaction-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "late-interaction-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn get_late_interaction_preset(_name: &str) -> Option<LateInteractionPreset> {
    None
}

/// Returns an empty list for builds without the `late-interaction-presets` feature.
///
/// Since v5.0.0.
#[cfg(not(feature = "late-interaction-presets"))]
#[cfg_attr(alef, alef(skip))]
pub fn list_late_interaction_presets() -> Vec<String> {
    Vec::new()
}

/// Caption a single image from bytes using a configured LLM.
///
/// # Arguments
///
/// * `image_bytes` - The image data.
/// * `llm_config` - LLM configuration for the VLM call.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// The generated caption text.
///
/// # Errors
///
/// Returns an error if the VLM call fails or if image format detection fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_image;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let image_bytes = std::fs::read("photo.jpg")?;
/// let config = LlmConfig {
///     model: "openai/gpt-4o-mini".to_string(),
///     ..Default::default()
/// };
/// let caption = caption_image(&image_bytes, &config, None).await?;
/// println!("Caption: {}", caption);
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "captioning", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use captioning::caption_image;

/// Caption a single image from a file path using a configured LLM.
///
/// # Arguments
///
/// * `path` - Path to the image file.
/// * `llm_config` - LLM configuration for the VLM call.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// The generated caption text.
///
/// # Errors
///
/// Returns an error if the file cannot be read, if image format detection fails,
/// or if the VLM call fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_image_file;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let config = LlmConfig {
///     model: "openai/gpt-4o-mini".to_string(),
///     ..Default::default()
/// };
/// let caption = caption_image_file("document_page_001.png", &config, None).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "captioning", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use captioning::caption_image_file;

/// Caption multiple images in a single batch.
///
/// Processes images sequentially (not in parallel). Returns one caption per input image
/// in the same order. If a caption fails, the error is returned immediately without
/// processing remaining images.
///
/// # Arguments
///
/// * `images` - Slice of image byte references to caption.
/// * `llm_config` - LLM configuration for the VLM calls.
/// * `custom_prompt` - Optional custom caption prompt. Uses the default
///   `RegionKind::Caption` prompt when `None`.
///
/// # Returns
///
/// A vector of captions, one per input image, in the same order.
///
/// # Errors
///
/// Returns an error if any VLM call fails.
///
/// # Example
///
/// ```ignore
/// use xberg::captioning::caption_images;
/// use xberg::LlmConfig;
///
/// # async fn example() -> xberg::Result<()> {
/// let image1 = std::fs::read("photo1.jpg")?;
/// let image2 = std::fs::read("photo2.jpg")?;
/// let images = vec![image1.as_ref(), image2.as_ref()];
/// let config = LlmConfig {
///     model: "openai/gpt-4o-mini".to_string(),
///     ..Default::default()
/// };
/// let captions = caption_images(&images, &config, None).await?;
/// assert_eq!(captions.len(), 2);
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "captioning", feature = "tokio-runtime"))]
#[cfg_attr(alef, alef(skip))]
pub use captioning::caption_images;

/// Unified post-extraction enrichment: classification, chunk classification, NER
/// and captioning in a single composable call. Transcription is not an enrichment
/// stage — it runs at extraction time via [`core::config::ExtractionConfig`].
pub mod enrich;
#[cfg_attr(alef, alef(skip))]
pub use enrich::enrich;
pub use enrich::{EnrichedResult, EnrichmentConfig};

#[cfg(feature = "ner")]
pub use enrich::NerEnrichmentConfig;

#[cfg(feature = "classification")]
pub use enrich::ClassificationEnrichmentConfig;

#[cfg(feature = "captioning")]
pub use enrich::CaptioningEnrichmentConfig;
