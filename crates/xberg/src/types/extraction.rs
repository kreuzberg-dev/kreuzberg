//! Core extraction types and results.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;

use super::djot::DjotContent;
use super::document_structure::DocumentStructure;
use super::metadata::Metadata;
use super::ocr_elements::OcrElement;
use super::page::PageContent;
use super::tables::Table;

/// How the extracted text was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// Text extracted directly from the document's native format (no OCR).
    Native,
    /// All text was obtained via OCR (e.g. scanned image-only PDF).
    Ocr,
    /// Text came from a combination of native extraction and OCR.
    Mixed,
}

impl ExtractionMethod {
    /// Returns the snake_case string representation of this method.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Ocr => "ocr",
            Self::Mixed => "mixed",
        }
    }

    /// Returns `true` if OCR was used at any stage of extraction.
    pub fn used_ocr(self) -> bool {
        !matches!(self, Self::Native)
    }

    pub(crate) fn from_metadata_value(value: &str) -> Option<Self> {
        match value {
            "native" => Some(Self::Native),
            "ocr" => Some(Self::Ocr),
            "mixed" => Some(Self::Mixed),
            _ => None,
        }
    }
}

/// Cheap structural counts for an extracted document.
///
/// Populated on every [`ExtractedDocument`] returned by `extract` /
/// `extract_batch`, regardless of whether the heavy `pages` / `images`
/// collections are materialized. A caller that only needs "how many pages /
/// tables / images did this document have?" (reporting, cost estimation,
/// progress, quotas) can read these without enabling per-page or per-image
/// extraction.
///
/// The page count comes from the parse (the extractor already walks the page
/// tree); it does not require opting into per-page content. `pages` is `0` for
/// inputs that are not page-addressable (e.g. plain text).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct DocumentCounts {
    /// Total pages in the source document (`0` when not page-addressable).
    pub pages: usize,
    /// Tables detected in the document.
    pub tables: usize,
    /// Images detected in the document.
    pub images: usize,
}

/// Structured per-language detection result: confidence, document share, and script —
/// the information the ISO-code-only `detected_languages` list cannot convey (#261).
///
/// Populated by [`crate::language_detection`] alongside `detected_languages`, with one
/// entry per language, in the same order as `detected_languages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct LanguageConfidence {
    /// ISO 639-3 language code, matching the corresponding entry in `detected_languages`.
    pub language: String,
    /// Confidence for this language, in `[0.0, 1.0]`.
    ///
    /// In single-language mode this is whatlang's `Info::confidence()` for the whole
    /// document. In multi-language mode this is the average whatlang confidence across
    /// the document's 200-character chunks that were classified as this language.
    pub confidence: f64,
    /// Share of the document's analyzed content classified as this language, in `[0.0, 1.0]`.
    ///
    /// In single-language mode this is always `1.0`. In multi-language mode this is the
    /// fraction of 200-character chunks classified as this language (chunks that did not
    /// meet `min_confidence` for any language are excluded from the count but still count
    /// toward the denominator).
    pub proportion: f64,
    /// Writing system whatlang detected for this language (e.g. `"Latin"`, `"Cyrillic"`).
    pub script: String,
    /// Whether this detection is considered reliable.
    ///
    /// In single-language mode this is whatlang's own `Info::is_reliable()` (confidence
    /// above whatlang's internal 0.9 threshold). In multi-language mode this is the
    /// chunk-averaged `confidence` above that same 0.9 threshold, since whatlang's
    /// `is_reliable()` only applies to a single detection.
    pub reliable: bool,
}

/// Document extracted by the core extraction pipeline.
///
/// `extract` and `extract_batch` return an `ExtractionResult` envelope whose
/// `results` field contains these per-document payloads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "api", schema(no_recursion))]
pub struct ExtractedDocument {
    /// Plain-text representation of the extracted document content.
    pub content: String,
    /// MIME type of the source document (e.g. `"application/pdf"`).
    #[cfg_attr(feature = "api", schema(value_type = String))]
    pub mime_type: Cow<'static, str>,
    /// Document-level metadata (author, title, dates, format-specific fields).
    pub metadata: Metadata,
    /// Extraction strategy used to produce the returned text.
    ///
    /// Populated when the extractor can reliably distinguish native text extraction,
    /// OCR-only extraction, or mixed native/OCR output.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub extraction_method: Option<ExtractionMethod>,
    /// Tables extracted from the document, each with structured cell data.
    pub tables: Vec<Table>,

    /// Cheap structural counts (pages, tables, images).
    ///
    /// Always populated by the extraction pipeline, even when the `pages` /
    /// `images` collections are `None`. See [`DocumentCounts`].
    #[serde(default)]
    pub counts: DocumentCounts,

    /// ISO 639-1 language codes detected in the document content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_languages: Option<Vec<String>>,

    /// Structured per-language detection results: confidence, document share, script,
    /// and reliability, alongside the ISO-code-only `detected_languages` (#261).
    ///
    /// One entry per language in `detected_languages`, in the same order. `None` under
    /// the same conditions as `detected_languages`: detection disabled, empty input
    /// text, or no language met the configured `min_confidence`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub detected_language_confidences: Option<Vec<LanguageConfidence>>,

    /// Text chunks when chunking is enabled.
    ///
    /// When chunking configuration is provided, the content is split into
    /// overlapping chunks for efficient processing. Each chunk contains the text,
    /// optional embeddings (if enabled), and metadata about its position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<Chunk>>,

    /// Extracted images from the document.
    ///
    /// When image extraction is enabled via `ImageExtractionConfig`, this field
    /// contains all images found in the document with their raw data and metadata.
    /// Each image may optionally contain a nested `ocr_result` if OCR was performed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ExtractedImage>>,

    /// Per-page content when page extraction is enabled.
    ///
    /// When page extraction is configured, the document is split into per-page content
    /// with tables and images mapped to their respective pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<PageContent>>,

    /// Semantic elements when element-based result format is enabled.
    ///
    /// When result_format is set to ElementBased, this field contains semantic
    /// elements with type classification, unique identifiers, and metadata for
    /// Unstructured-compatible element-based processing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub elements: Option<Vec<Element>>,

    /// Rich Djot content structure (when extracting Djot documents).
    ///
    /// When extracting Djot documents with structured extraction enabled,
    /// this field contains the full semantic structure including:
    /// - Block-level elements with nesting
    /// - Inline formatting with attributes
    /// - Links, images, footnotes
    /// - Math expressions
    /// - Complete attribute information
    ///
    /// The `content` field still contains plain text for backward compatibility.
    ///
    /// Always `None` for non-Djot documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub djot_content: Option<DjotContent>,

    /// OCR elements with full spatial and confidence metadata.
    ///
    /// When OCR is performed with element extraction enabled, this field contains
    /// the structured representation of detected text including:
    /// - Bounding geometry (rectangles or quadrilaterals)
    /// - Confidence scores (detection and recognition)
    /// - Rotation information
    /// - Hierarchical relationships (Tesseract only)
    ///
    /// This field preserves all metadata that would otherwise be lost when
    /// converting to plain text or markdown output formats.
    ///
    /// Only populated when `OcrElementConfig.include_elements` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub ocr_elements: Option<Vec<OcrElement>>,

    /// Structured document tree (when document structure extraction is enabled).
    ///
    /// When `include_document_structure` is true in `ExtractionConfig`, this field
    /// contains the full hierarchical representation of the document including:
    /// - Heading-driven section nesting
    /// - Table grids with cell-level metadata
    /// - Content layer classification (body, header, footer, footnote)
    /// - Inline text annotations (formatting, links)
    /// - Bounding boxes and page numbers
    ///
    /// Independent of `result_format` — can be combined with Unified or ElementBased.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub document: Option<DocumentStructure>,

    /// Extracted keywords when keyword extraction is enabled.
    ///
    /// When keyword extraction (RAKE or YAKE) is configured, this field contains
    /// the extracted keywords with scores, algorithm info, and position data.
    /// Previously stored in `metadata.additional["keywords"]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
    pub extracted_keywords: Option<Vec<crate::keywords::Keyword>>,

    /// Document quality score from quality analysis.
    ///
    /// A value between 0.0 and 1.0 indicating the overall text quality.
    /// Previously stored in `metadata.additional["quality_score"]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub quality_score: Option<f64>,

    /// Non-fatal warnings collected during processing pipeline stages.
    ///
    /// Captures errors from optional pipeline features (embedding, chunking,
    /// language detection, output formatting) that don't prevent extraction
    /// but may indicate degraded results.
    /// Previously stored as individual keys in `metadata.additional`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub processing_warnings: Vec<ProcessingWarning>,

    /// PDF annotations extracted from the document.
    ///
    /// When annotation extraction is enabled via `PdfConfig::extract_annotations`,
    /// this field contains text notes, highlights, links, stamps, and other
    /// annotations found in PDF documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub annotations: Option<Vec<super::annotations::PdfAnnotation>>,

    /// Nested extraction results from archive contents.
    ///
    /// When extracting archives, each processable file inside produces its own
    /// full extraction result. Set to `None` for non-archive formats.
    /// Use `max_archive_depth` in config to control recursion depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub children: Option<Vec<ArchiveEntry>>,

    /// URIs/links discovered during document extraction.
    ///
    /// Contains hyperlinks, image references, citations, email addresses, and
    /// other URI-like references found in the document. Always extracted when
    /// present in the source document.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub uris: Option<Vec<super::uri::ExtractedUri>>,

    /// Tracked changes embedded in the source document.
    ///
    /// Populated by per-format extractors that understand change-tracking
    /// metadata (DOCX `w:ins`/`w:del`/`w:rPrChange`, ODT `text:change-*`,
    /// …). Every extractor defaults to `None` until its format-specific
    /// implementation is added. Extractors that do populate this field follow
    /// the "accepted-changes" convention: inserted text is present in
    /// `content`, deleted text is absent — the revision list is the separate
    /// audit trail.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub revisions: Option<Vec<super::revisions::DocumentRevision>>,

    /// Structured extraction output from LLM-based JSON schema extraction.
    ///
    /// When `structured_extraction` is configured in `ExtractionConfig`, the
    /// extracted document content is sent to a VLM with the provided JSON schema.
    /// The response is parsed and stored here as a JSON value matching the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,

    /// Code intelligence results from tree-sitter analysis.
    ///
    /// Populated when extracting source code files with the `tree-sitter` feature.
    /// Contains metrics, structural analysis, imports/exports, comments,
    /// docstrings, symbols, diagnostics, and optionally chunked code segments.
    ///
    /// Stored as an opaque JSON value so that all language bindings (Go, Java,
    /// C#, …) can deserialize it as a raw JSON object rather than a typed struct.
    /// The underlying type is `tree_sitter_language_pack::ProcessResult`.
    #[cfg(feature = "tree-sitter")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_intelligence: Option<serde_json::Value>,

    /// LLM token usage and cost data for all LLM calls made during this extraction.
    ///
    /// Contains one entry per LLM call. Multiple entries are produced when
    /// VLM OCR, structured extraction, or LLM embeddings run during
    /// the same extraction.
    ///
    /// `None` when no LLM was used.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub llm_usage: Option<Vec<LlmUsage>>,

    /// Named entities detected in `content` by the NER post-processor.
    ///
    /// `None` when no NER backend is configured. Populated by the `xberg-gliner`
    /// ONNX backend or the LLM-driven backend (see `crates/xberg/src/text/ner/`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub entities: Option<Vec<super::entity::Entity>>,

    /// Summary of `content` produced by the summarisation post-processor.
    ///
    /// `None` when summarisation is not configured. Populated by the TextRank
    /// extractive backend (deterministic, no external service) or by the
    /// liter-llm-driven abstractive backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub summary: Option<super::summary::DocumentSummary>,

    /// Confidence score computed by the heuristics pipeline.
    ///
    /// Populated when the `heuristics` feature is enabled and confidence
    /// scoring has been performed.  Combines text-coverage, OCR aggregate
    /// confidence, and schema-compliance into a single `[0, 1]` value.
    ///
    /// `None` when confidence scoring is not configured or the feature is
    /// absent.
    #[cfg(feature = "heuristics")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub extraction_confidence: Option<crate::heuristics::confidence::ExtractionConfidence>,

    /// Translation of `content` produced by the translation post-processor.
    ///
    /// `None` when translation is not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub translation: Option<super::translation::Translation>,

    /// Per-page classifications produced by the page-classification post-processor.
    ///
    /// `None` when classification is not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub page_classifications: Option<Vec<super::classification::PageClassification>>,

    /// Audit report of redactions applied by the redaction post-processor.
    ///
    /// The redaction processor rewrites `content`, `formatted_content`, every
    /// chunk's text, and the textual fields of `entities` / `summary` / `translation` /
    /// `page_classifications` in place. This report describes what was found and how it
    /// was replaced. `None` when redaction is not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub redaction_report: Option<super::redaction::RedactionReport>,

    /// Mathematical formulas recognized in the document.
    ///
    /// Populated from every source that produces formulas: layout-guided OCR
    /// (with geometry), VLM OCR (text only), and markup extraction (DOCX,
    /// PPTX, ODT, EPUB, HTML, JATS, LaTeX, Markdown, and related formats,
    /// without geometry). Empty when the document contains no formulas.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub formulas: Vec<super::formula::Formula>,

    /// Form fields extracted from a PDF's AcroForm or XFA structure.
    ///
    /// Populated by the PDF extractor when `PdfConfig::extract_form_fields` is
    /// enabled (default) and the document is a fillable form. Empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub form_fields: Vec<super::form_field::PdfFormField>,

    /// Pre-rendered content in the requested output format.
    ///
    /// Pipeline-internal scratch space, not a result field. `derive_extraction_result`
    /// renders it before tree derivation consumes the element data, post-processors may
    /// rewrite it alongside `content`, and `apply_output_format` then *moves* it into
    /// `content` as the last pipeline step. Every document returned by `extract_bytes` /
    /// `extract_file` — and every nested archive or email child — therefore carries
    /// `None` here, with the rendering in `content`.
    ///
    /// It stays `pub` because it is part of the Rust plugin contract: a
    /// `DocumentExtractor` may return it pre-rendered, and a post-processor that rewrites
    /// `content` must rewrite this alongside it or the rendering is discarded as stale
    /// (see `core::pipeline::discard_diverged_formatted_content`). It is hidden from the
    /// language bindings, which only ever observe the post-pipeline document.
    #[serde(skip)]
    #[cfg_attr(alef, alef(skip))]
    pub formatted_content: Option<String>,

    /// Structured hOCR document for the OCR+layout pipeline.
    ///
    /// When tesseract produces hOCR output, the parsed `InternalDocument` carries
    /// paragraph structure with bounding boxes and confidence scores. The layout
    /// classification step enriches these elements before final rendering.
    #[serde(skip)]
    #[allow(dead_code)]
    #[cfg_attr(alef, alef(skip))]
    pub(crate) ocr_internal_document: Option<super::internal::InternalDocument>,

    /// The original `InternalDocument` from the extractor, preserved before derivation.
    ///
    /// Stored by the pipeline before `derive_extraction_result` consumes the document, so
    /// that downstream transformation steps (element-based result format) can walk the
    /// extractor's native reading order instead of reassembling from per-page content.
    /// This is especially important for DOCX, which has no native page boundaries: the
    /// per-page reconstruction scrambles element order, but the flat element list in the
    /// `InternalDocument` is always in reading order.
    ///
    /// `None` for extraction paths that do not go through the async/sync pipeline
    /// (e.g., direct `ExtractedDocument::from_ocr` construction).
    #[serde(skip)]
    #[allow(dead_code)]
    #[cfg_attr(alef, alef(skip))]
    pub(crate) internal_document: Option<super::internal::InternalDocument>,
}

/// A single file extracted from an archive.
///
/// When archives (ZIP, TAR, 7Z, GZIP) are extracted with recursive extraction
/// enabled, each processable file produces its own full `ExtractedDocument`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ArchiveEntry {
    /// Archive-relative file path (e.g. "folder/document.pdf").
    pub path: String,
    /// Detected MIME type of the file.
    pub mime_type: String,
    /// Full extraction result for this file.
    pub result: Box<ExtractedDocument>,
}

/// A non-fatal warning from a processing pipeline stage.
///
/// Captures errors from optional features that don't prevent extraction
/// but may indicate degraded results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ProcessingWarning {
    /// The pipeline stage or feature that produced this warning
    /// (e.g., "embedding", "chunking", "language_detection", "output_format").
    #[cfg_attr(feature = "api", schema(value_type = String))]
    pub source: Cow<'static, str>,
    /// Human-readable description of what went wrong.
    #[cfg_attr(feature = "api", schema(value_type = String))]
    pub message: Cow<'static, str>,
}

/// Token usage and cost data for a single LLM call made during extraction.
///
/// Populated when VLM OCR, structured extraction, or LLM-based embeddings
/// are used. Multiple entries may be present when multiple LLM calls occur
/// within one extraction (e.g. VLM OCR + structured extraction).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LlmUsage {
    /// The LLM model identifier (e.g. "openai/gpt-4o", "anthropic/claude-sonnet-4-20250514").
    pub model: String,
    /// The pipeline stage that triggered this LLM call
    /// (e.g. "vlm_ocr", "structured_extraction", "embeddings").
    pub source: String,
    /// Number of input/prompt tokens consumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Number of output/completion tokens generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Total tokens (input + output).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Estimated cost in USD based on the provider's published pricing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<f64>,
    /// Why the model stopped generating (e.g. "stop", "length", "content_filter").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// Semantic structural classification of a text chunk.
///
/// Assigned by the heuristic classifier in `chunking::classifier`.
/// Defaults to `Unknown` when no rule matches.
/// Designed to be extended in future versions without breaking changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChunkType {
    /// Section heading or document title.
    Heading,
    /// Party list: names, addresses, and signatories.
    PartyList,
    /// Definition clause ("X means…", "X shall mean…").
    Definitions,
    /// Operative clause containing legal/contractual action verbs.
    OperativeClause,
    /// Signature block with signatures, names, and dates.
    SignatureBlock,
    /// Schedule, annex, appendix, or exhibit section.
    Schedule,
    /// Table-like content with aligned columns or repeated patterns.
    TableLike,
    /// Mathematical formula or equation.
    Formula,
    /// Code block or preformatted content.
    CodeBlock,
    /// Function or method definition (tree-sitter structured code chunking).
    Function,
    /// Class, struct, interface, or trait definition (tree-sitter structured code chunking).
    Class,
    /// Module, namespace, or top-level file scope (tree-sitter structured code chunking).
    Module,
    /// Embedded or referenced image content.
    Image,
    /// Organizational chart or hierarchy diagram.
    OrgChart,
    /// Diagram, figure, or visual illustration.
    Diagram,
    /// Unclassified or mixed content.
    #[default]
    Unknown,
}

/// A text chunk with optional embedding and metadata.
///
/// Chunks are created when chunking is enabled in `ExtractionConfig`. Each chunk
/// contains the text content, optional embedding vector (if embedding generation
/// is configured), and metadata about its position in the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct Chunk {
    /// The text content of this chunk.
    pub content: String,

    /// Semantic structural classification of this chunk.
    ///
    /// Assigned by the heuristic classifier based on content patterns and
    /// heading context. Defaults to `ChunkType::Unknown` when no rule matches.
    #[serde(default)]
    pub chunk_type: ChunkType,

    /// Optional embedding vector for this chunk.
    ///
    /// Only populated when `EmbeddingConfig` is provided in chunking configuration.
    /// The dimensionality depends on the chosen embedding model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,

    /// Optional sparse (SPLADE) learned embedding for this chunk.
    ///
    /// Only populated when sparse-embedding generation is configured for chunking.
    /// `None` otherwise, including on builds without the `sparse-embeddings` feature.
    ///
    /// Uses the crate-root [`crate::SparseEmbedding`] alias rather than
    /// `crate::sparse_embeddings::SparseEmbedding` directly: the `sparse_embeddings`
    /// module itself only compiles under `sparse-embeddings`/`sparse-embedding-presets`,
    /// while the crate-root alias is always defined (a field-compatible stub on builds
    /// without either feature), so this field — and `Chunk` itself — compiles on every
    /// feature combination, including the crate's default features.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub sparse_embedding: Option<crate::SparseEmbedding>,

    /// Optional ColBERT-style multi-vector (late-interaction) embedding for this chunk.
    ///
    /// Only populated when late-interaction embedding generation is configured for
    /// chunking. `None` otherwise, including on builds without the `late-interaction`
    /// feature.
    ///
    /// Uses the crate-root [`crate::MultiVectorEmbedding`] alias for the same reason
    /// `sparse_embedding` uses [`crate::SparseEmbedding`] — see that field's docs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub late_interaction: Option<crate::MultiVectorEmbedding>,

    /// Metadata about this chunk's position and properties.
    pub metadata: ChunkMetadata,
}

/// Heading context for a chunk within a Markdown document.
///
/// Contains the heading hierarchy from document root to this chunk's section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct HeadingContext {
    /// The heading hierarchy from document root to this chunk's section.
    /// Index 0 is the outermost (h1), last element is the most specific.
    pub headings: Vec<HeadingLevel>,
}

/// A single heading in the hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct HeadingLevel {
    /// Heading depth (1 = h1, 2 = h2, etc.)
    pub level: u8,
    /// The text content of the heading.
    pub text: String,
}

/// Metadata about a chunk's position in the original document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ChunkMetadata {
    /// Byte offset where this chunk starts in the original text (UTF-8 valid boundary).
    pub byte_start: usize,

    /// Byte offset where this chunk ends in the original text (UTF-8 valid boundary).
    pub byte_end: usize,

    /// Number of tokens in this chunk (if available).
    ///
    /// This is calculated by the embedding model's tokenizer if embeddings are enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_count: Option<usize>,

    /// Zero-based index of this chunk in the document.
    pub chunk_index: usize,

    /// Total number of chunks in the document.
    pub total_chunks: usize,

    /// First page number this chunk spans (1-indexed).
    ///
    /// Only populated when page tracking is enabled in extraction configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_page: Option<u32>,

    /// Last page number this chunk spans (1-indexed, equal to first_page for single-page chunks).
    ///
    /// Only populated when page tracking is enabled in extraction configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_page: Option<u32>,

    /// Heading context when using Markdown chunker.
    ///
    /// Contains the heading hierarchy this chunk falls under.
    /// Only populated when `ChunkerType::Markdown` is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub heading_context: Option<HeadingContext>,

    /// Flattened heading trail from document root to this chunk's section.
    ///
    /// Each element is a heading's text, outermost first. Derived from
    /// [`heading_context`](Self::heading_context) when present; empty otherwise.
    /// Provides a binding-friendly, RAG-shaped breadcrumb without requiring
    /// callers to walk the nested [`HeadingContext`] structure.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub heading_path: Vec<String>,

    /// Indices into `ExtractedDocument.images` for images on pages covered by this chunk.
    ///
    /// Contains zero-based indices into the top-level `images` collection for every
    /// image whose `page_number` falls within `[first_page, last_page]`.
    /// Empty when image extraction is disabled or the chunk spans no pages with images.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub image_indices: Vec<u32>,

    /// Ids of the [`DocumentNode`](super::document_structure::DocumentNode)s
    /// this chunk was derived from.
    ///
    /// Joins a chunk back to the structured document tree via
    /// [`DocumentNode::id`](super::document_structure::DocumentNode::id).
    /// Empty until the node-to-rendered-offset mapping needed to compute the
    /// intersection is implemented (tracked under #1294/#1295); this field is
    /// the wire-format foundation for that follow-up.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub node_ids: Vec<String>,

    /// Per-page bounding-box spans this chunk covers, for viewer highlighting (#1295).
    ///
    /// One entry per page the chunk overlaps, in page order — the first and last entries'
    /// `page` fields equal [`first_page`](Self::first_page)/[`last_page`](Self::last_page).
    /// Populated whenever page-boundary provenance is available (the same condition under
    /// which `first_page`/`last_page` are populated); each entry's `bbox` is additionally
    /// populated when the document's structured node tree ([`ExtractedDocument::document`]) is
    /// available, as the union of that page's body-layer node bounding boxes found within this
    /// chunk. Empty when page-boundary provenance is unavailable (mirrors `first_page`/
    /// `last_page` being `None`).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub page_spans: Vec<PageSpan>,

    /// Multi-label classification result for this chunk.
    ///
    /// Populated by the chunk-classification post-processor when
    /// [`ExtractionConfig::chunk_classification`](crate::core::config::ExtractionConfig::chunk_classification)
    /// is set. A chunk may match zero, one, or many of the configured label
    /// definitions. Empty when chunk classification was not configured.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub classifications: Vec<super::classification::ClassificationLabel>,
}

/// A single page covered by a chunk, with an optional bounding box on that page.
///
/// See [`ChunkMetadata::page_spans`] (#1295) for population semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageSpan {
    /// Page number (1-indexed).
    pub page: u32,

    /// Bounding box on this page, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BoundingBox>,
}

/// Heuristic classification of what an image likely depicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    /// Photographic image (natural scene, photograph)
    Photograph,
    /// Technical or schematic diagram
    Diagram,
    /// Chart, graph, or plot
    Chart,
    /// Freehand or technical drawing
    Drawing,
    /// Text-heavy image (scanned text, document)
    TextBlock,
    /// Decorative element or border
    Decoration,
    /// Logo or brand mark
    Logo,
    /// Small icon
    Icon,
    /// Fragment of a larger tiled image (tile of a technical drawing)
    TileFragment,
    /// Mask or transparency map
    Mask,
    /// Full-page render produced during OCR preprocessing; used as a citation thumbnail.
    PageRaster,
    /// Could not classify with reasonable confidence
    Unknown,
}

/// Extracted image from a document.
///
/// Contains raw image data, metadata, and optional nested OCR results.
/// Raw bytes allow cross-language compatibility - users can convert to
/// PIL.Image (Python), Sharp (Node.js), or other formats as needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ExtractedImage {
    /// Raw image data (PNG, JPEG, WebP, etc. bytes).
    /// Uses `bytes::Bytes` for cheap cloning of large buffers.
    #[cfg_attr(feature = "api", schema(value_type = Vec<u8>, format = "binary"))]
    pub data: Bytes,

    /// Image format (e.g., "jpeg", "png", "webp")
    /// Uses Cow<'static, str> to avoid allocation for static literals.
    #[cfg_attr(feature = "api", schema(value_type = String))]
    pub format: Cow<'static, str>,

    /// Zero-indexed position of this image in the document/page
    pub image_index: u32,

    /// Page/slide number where image was found (1-indexed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_number: Option<u32>,

    /// Image width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Image height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Colorspace information (e.g., "RGB", "CMYK", "Gray")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colorspace: Option<String>,

    /// Bits per color component (e.g., 8, 16)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits_per_component: Option<u32>,

    /// Whether this image is a mask image
    #[serde(default)]
    pub is_mask: bool,

    /// Optional description of the image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Nested OCR extraction result (if image was OCRed)
    ///
    /// When OCR is performed on this image, the result is embedded here
    /// rather than in a separate collection, making the relationship explicit.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "api", schema(value_type = Option<ExtractedDocument>))]
    pub ocr_result: Option<Box<ExtractedDocument>>,

    /// Bounding box of the image on the page (PDF coordinates: x0=left, y0=bottom, x1=right, y1=top).
    /// Only populated for PDF-extracted images when position data is available from the PDF extractor.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub bounding_box: Option<BoundingBox>,

    /// Original source path of the image within the document archive (e.g., "media/image1.png" in DOCX).
    /// Used for rendering image references when the binary data is not extracted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub source_path: Option<String>,

    /// Heuristic classification of what this image likely depicts.
    /// `None` if classification was disabled or inconclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_kind: Option<ImageKind>,

    /// Confidence score for `image_kind`, in the range 0.0 to 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind_confidence: Option<f32>,

    /// Identifier shared across images that form a single logical figure
    /// (e.g. all raster tiles of one technical drawing). `None` for singletons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<u32>,

    /// VLM-generated caption describing the image, when captioning is configured.
    ///
    /// Populated by the captioning post-processor
    /// (`crates/xberg/src/plugins/processor/builtin/captioning.rs`), which routes
    /// each image through `crate::llm::region_extractor::extract_region_with_vlm` in
    /// caption mode. `None` when captioning is disabled or the VLM declined to caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,

    /// QR codes decoded from this image, when QR detection is enabled.
    ///
    /// Populated by the QR post-processor (`crates/xberg/src/extractors/qr.rs`) via
    /// the pure-Rust `rqrr` decoder. `None` when QR detection is disabled; an empty
    /// `Some(vec![])` when detection ran but found nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_codes: Option<Vec<super::qr::QrCode>>,

    /// Base64-encoded copy of `data`; populated when `ImageExtractionConfig::include_data_base64`
    /// is `true`. Omitted from JSON by default; use instead of `data` in JSON-only clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

/// Result-shape selection for extraction results.
///
/// Distinct from [`crate::OutputFormat`] (which controls rendering — Plain, Markdown,
/// HTML, etc.). `ResultFormat` controls the *shape* of the result: a unified content
/// blob vs. an element-based decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResultFormat {
    /// Unified format with all content in `content` field
    #[default]
    Unified,
    /// Element-based format with semantic element extraction
    ElementBased,
}
#[cfg_attr(alef, alef(skip))]
/// Unique identifier for semantic elements.
///
/// Wraps a string identifier that is deterministically generated
/// from element type, content, and page number.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "api", schema(value_type = String))]
pub struct ElementId(String);

impl ElementId {
    /// Create a new ElementId from a string.
    ///
    /// # Errors
    ///
    /// Returns error if the string is not valid.
    pub(crate) fn new(hex_str: impl Into<String>) -> std::result::Result<Self, String> {
        let s = hex_str.into();
        if s.is_empty() {
            return Err("ElementId cannot be empty".to_string());
        }
        Ok(ElementId(s))
    }
}

impl AsRef<str> for ElementId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ElementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Semantic element type classification.
///
/// Categorizes text content into semantic units for downstream processing.
/// Supports the element types commonly found in Unstructured documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ElementType {
    /// Document title
    Title,
    /// Main narrative text body
    NarrativeText,
    /// Section heading
    Heading,
    /// List item (bullet, numbered, etc.)
    ListItem,
    /// Table element
    Table,
    /// Image element
    Image,
    /// Page break marker
    PageBreak,
    /// Code block
    CodeBlock,
    /// Mathematical formula (LaTeX source in `text`)
    Formula,
    /// Block quote
    BlockQuote,
    /// Footer text
    Footer,
    /// Header text
    Header,
}
/// Bounding box coordinates for element positioning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct BoundingBox {
    /// Left x-coordinate
    pub x0: f64,
    /// Bottom y-coordinate
    pub y0: f64,
    /// Right x-coordinate
    pub x1: f64,
    /// Top y-coordinate
    pub y1: f64,
}

/// Metadata for a semantic element.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ElementMetadata {
    /// Page number (1-indexed)
    pub page_number: Option<u32>,
    /// Source filename or document name
    pub filename: Option<String>,
    /// Bounding box coordinates if available
    pub coordinates: Option<BoundingBox>,
    /// Position index in the element sequence
    pub element_index: Option<usize>,
    /// Additional custom metadata
    pub additional: HashMap<String, String>,
}

/// Semantic element extracted from document.
///
/// Represents a logical unit of content with semantic classification,
/// unique identifier, and metadata for tracking origin and position.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct Element {
    /// Unique element identifier
    #[cfg_attr(alef, alef(skip))]
    #[serde(skip)]
    pub element_id: ElementId,
    /// Semantic type of this element
    pub element_type: ElementType,
    /// Text content of the element
    pub text: String,
    /// Metadata about the element
    pub metadata: ElementMetadata,
}

impl ExtractedDocument {
    /// Convert from an OCR result.
    #[cfg_attr(alef, alef(skip))]
    pub fn from_ocr(ocr: super::formats::OcrExtractionResult) -> Self {
        Self {
            content: ocr.content,
            mime_type: Cow::Owned(ocr.mime_type),
            extraction_method: Some(ExtractionMethod::Ocr),
            tables: ocr.tables.into_iter().map(super::tables::Table::from_ocr).collect(),
            ocr_elements: ocr.ocr_elements,
            ..Default::default()
        }
    }
}

impl super::tables::Table {
    /// Convert from an OCR table result.
    pub fn from_ocr(ocr: super::formats::OcrTable) -> Self {
        Self {
            cells: ocr.cells,
            markdown: ocr.markdown,
            page_number: ocr.page_number,
            bounding_box: ocr.bounding_box.map(|b| super::extraction::BoundingBox {
                x0: b.left as f64,
                y0: b.top as f64,
                x1: b.right as f64,
                y1: b.bottom as f64,
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_metadata_omitting_heading_path_deserializes_to_empty_vec() {
        // heading_path has `#[serde(default)]` — stored JSON without the field
        let json = r#"{
            "byte_start": 0,
            "byte_end": 42,
            "chunk_index": 0,
            "total_chunks": 1
        }"#;
        let meta: ChunkMetadata = serde_json::from_str(json).unwrap();
        assert!(
            meta.heading_path.is_empty(),
            "omitted heading_path must default to empty vec, got: {:?}",
            meta.heading_path
        );
    }

    #[test]
    fn extraction_result_omitting_formulas_and_form_fields_defaults_to_empty() {
        // Both `formulas` and `form_fields` use `#[serde(default)]` and
        let json = r#"{
            "content": "hello",
            "mime_type": "text/plain",
            "metadata": {},
            "tables": []
        }"#;
        let result: ExtractedDocument = serde_json::from_str(json).unwrap();
        assert!(result.formulas.is_empty(), "omitted formulas must default to empty vec");
        assert!(
            result.form_fields.is_empty(),
            "omitted form_fields must default to empty vec"
        );
    }

    #[test]
    fn extraction_result_omitting_counts_defaults_to_zero() {
        // `counts` uses `#[serde(default)]`; stored JSON predating the field must
        let json = r#"{
            "content": "hello",
            "mime_type": "text/plain",
            "metadata": {},
            "tables": []
        }"#;
        let result: ExtractedDocument = serde_json::from_str(json).unwrap();
        assert_eq!(
            result.counts,
            DocumentCounts::default(),
            "omitted counts must default to all-zero DocumentCounts"
        );
    }

    #[test]
    fn document_counts_round_trip() {
        let counts = DocumentCounts {
            pages: 7,
            tables: 3,
            images: 2,
        };
        let json = serde_json::to_string(&counts).unwrap();
        let back: DocumentCounts = serde_json::from_str(&json).unwrap();
        assert_eq!(counts, back);
    }

    fn empty_chunk_metadata() -> ChunkMetadata {
        ChunkMetadata {
            byte_start: 0,
            byte_end: 10,
            token_count: None,
            chunk_index: 0,
            total_chunks: 1,
            first_page: None,
            last_page: None,
            heading_context: None,
            heading_path: Vec::new(),
            image_indices: Vec::new(),
            node_ids: Vec::new(),
            page_spans: Vec::new(),
            classifications: Vec::new(),
        }
    }

    #[test]
    fn chunk_metadata_node_ids_omitted_when_empty() {
        let meta = empty_chunk_metadata();
        let json = serde_json::to_value(&meta).expect("serialize");
        assert!(
            json.get("node_ids").is_none(),
            "empty node_ids must be omitted from the wire, got: {json:?}"
        );
    }

    #[test]
    fn chunk_metadata_node_ids_present_when_set() {
        let mut meta = empty_chunk_metadata();
        meta.node_ids = vec![
            crate::types::document_structure::NodeId::generate("paragraph", "a", Some(1), 0).to_string(),
            crate::types::document_structure::NodeId::generate("paragraph", "b", Some(1), 1).to_string(),
        ];
        let json = serde_json::to_value(&meta).expect("serialize");
        let ids = json
            .get("node_ids")
            .expect("node_ids present")
            .as_array()
            .expect("array");
        assert_eq!(ids.len(), 2);
        assert!(ids[0].is_string(), "node ids must serialize as bare strings");

        let back: ChunkMetadata = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.node_ids, meta.node_ids);
    }

    #[test]
    fn chunk_metadata_omitting_node_ids_deserializes_to_empty_vec() {
        let json = r#"{
            "byte_start": 0,
            "byte_end": 42,
            "chunk_index": 0,
            "total_chunks": 1
        }"#;
        let meta: ChunkMetadata = serde_json::from_str(json).unwrap();
        assert!(meta.node_ids.is_empty(), "omitted node_ids must default to empty vec");
    }

    #[test]
    fn chunk_metadata_page_spans_omitted_when_empty() {
        let meta = empty_chunk_metadata();
        let json = serde_json::to_value(&meta).expect("serialize");
        assert!(
            json.get("page_spans").is_none(),
            "empty page_spans must be omitted from the wire, got: {json:?}"
        );
    }

    #[test]
    fn chunk_metadata_page_spans_present_when_set() {
        let mut meta = empty_chunk_metadata();
        meta.page_spans = vec![
            PageSpan {
                page: 1,
                bbox: Some(BoundingBox {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 100.0,
                    y1: 200.0,
                }),
            },
            PageSpan { page: 2, bbox: None },
        ];
        let json = serde_json::to_value(&meta).expect("serialize");
        let spans = json
            .get("page_spans")
            .expect("page_spans present")
            .as_array()
            .expect("array");
        assert_eq!(spans.len(), 2);
        assert!(spans[0].get("bbox").is_some());
        assert!(spans[1].get("bbox").is_none(), "None bbox must be omitted per-span");

        let back: ChunkMetadata = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.page_spans, meta.page_spans);
    }

    #[test]
    fn chunk_metadata_omitting_page_spans_deserializes_to_empty_vec() {
        let json = r#"{
            "byte_start": 0,
            "byte_end": 42,
            "chunk_index": 0,
            "total_chunks": 1
        }"#;
        let meta: ChunkMetadata = serde_json::from_str(json).unwrap();
        assert!(
            meta.page_spans.is_empty(),
            "omitted page_spans must default to empty vec"
        );
    }

    #[test]
    fn extraction_result_formula_round_trip() {
        use super::super::formula::Formula;

        let formula = Formula {
            latex: r"E = mc^2".to_string(),
            bbox: Some(BoundingBox {
                x0: 10.0,
                y0: 20.0,
                x1: 100.0,
                y1: 50.0,
            }),
            page: Some(1),
        };

        let result = ExtractedDocument {
            content: "Physics document".to_string(),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            formulas: vec![formula],
            ..Default::default()
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("formulas"), "non-empty formulas must be serialized");

        let deserialized: ExtractedDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.formulas.len(), 1);
        assert_eq!(deserialized.formulas[0].latex, r"E = mc^2");
        assert_eq!(deserialized.formulas[0].page, Some(1));
        assert_eq!(deserialized.formulas[0].bbox.unwrap().x0, 10.0);
    }

    #[test]
    fn formula_without_geometry_round_trips_and_omits_keys() {
        use super::super::formula::Formula;

        let formula = Formula {
            latex: "x + 1".to_string(),
            bbox: None,
            page: None,
        };
        let json = serde_json::to_string(&formula).unwrap();
        assert!(!json.contains("bbox"), "absent bbox must be omitted: {json}");
        assert!(!json.contains("page"), "absent page must be omitted: {json}");

        let back: Formula = serde_json::from_str(r#"{"latex":"x + 1"}"#).unwrap();
        assert_eq!(back.bbox, None);
        assert_eq!(back.page, None);
        assert_eq!(back, formula);
    }

    #[test]
    fn extraction_result_pdf_form_field_round_trip() {
        use super::super::form_field::{FormFieldType, PdfFormField};

        let field = PdfFormField {
            name: "FirstName".to_string(),
            full_name: "PersonalInfo.FirstName".to_string(),
            field_type: FormFieldType::Text,
            value: Some("Alice".to_string()),
            default_value: None,
            flags: 0,
            page: Some(1),
            bbox: Some(BoundingBox {
                x0: 72.0,
                y0: 300.0,
                x1: 300.0,
                y1: 320.0,
            }),
            max_length: Some(50),
            tooltip: Some("Enter your first name".to_string()),
        };

        let result = ExtractedDocument {
            content: "Form document".to_string(),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            form_fields: vec![field],
            ..Default::default()
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("form_fields"), "non-empty form_fields must be serialized");

        let deserialized: ExtractedDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.form_fields.len(), 1);
        assert_eq!(deserialized.form_fields[0].name, "FirstName");
        assert_eq!(deserialized.form_fields[0].full_name, "PersonalInfo.FirstName");
        assert_eq!(deserialized.form_fields[0].field_type, FormFieldType::Text);
        assert_eq!(deserialized.form_fields[0].value.as_deref(), Some("Alice"));
        assert_eq!(deserialized.form_fields[0].max_length, Some(50));
        let bbox = deserialized.form_fields[0].bbox.unwrap();
        assert_eq!(bbox.x0, 72.0);
        assert_eq!(bbox.y1, 320.0);
    }

    fn empty_chunk(content: &str) -> Chunk {
        Chunk {
            content: content.to_string(),
            chunk_type: ChunkType::default(),
            embedding: None,
            sparse_embedding: None,
            late_interaction: None,
            metadata: empty_chunk_metadata(),
        }
    }

    #[test]
    fn should_round_trip_exact_sparse_and_late_interaction_vectors_when_populated() {
        let mut chunk = empty_chunk("hello world");
        chunk.sparse_embedding = Some(crate::SparseEmbedding {
            indices: vec![3, 7, 42],
            values: vec![0.5, 0.25, 0.125],
        });
        chunk.late_interaction = Some(crate::MultiVectorEmbedding {
            num_tokens: 2,
            dim: 3,
            data: vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
        });

        let json = serde_json::to_string(&chunk).expect("serialize");
        let back: Chunk = serde_json::from_str(&json).expect("deserialize");

        let sparse = back.sparse_embedding.expect("sparse_embedding must round-trip as Some");
        assert_eq!(sparse.indices, vec![3, 7, 42]);
        assert_eq!(sparse.values, vec![0.5, 0.25, 0.125]);

        let late = back.late_interaction.expect("late_interaction must round-trip as Some");
        assert_eq!(late.num_tokens, 2);
        assert_eq!(late.dim, 3);
        assert_eq!(late.data, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    }

    #[test]
    fn should_omit_sparse_and_late_interaction_fields_when_not_configured() {
        let chunk = empty_chunk("hello world");
        assert!(chunk.sparse_embedding.is_none());
        assert!(chunk.late_interaction.is_none());

        let json = serde_json::to_value(&chunk).expect("serialize");
        assert!(
            json.get("sparse_embedding").is_none(),
            "sparse_embedding must be omitted from the wire when None, got: {json:?}"
        );
        assert!(
            json.get("late_interaction").is_none(),
            "late_interaction must be omitted from the wire when None, got: {json:?}"
        );

        let back: Chunk = serde_json::from_value(json).expect("deserialize");
        assert!(back.sparse_embedding.is_none());
        assert!(back.late_interaction.is_none());
    }
}
