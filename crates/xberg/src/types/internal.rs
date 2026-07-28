//! Internal flat document representation.
//!
//! This module provides the internal DTO that all extractors output. It is a flat,
//! append-only structure optimized for extraction performance. The public
//! [`DocumentStructure`](super::document_structure::DocumentStructure) tree and
//! relationship graph are derived from this in a post-processing step.
//!
//! # Design
//!
//! - **Flat `Vec<InternalElement>`**: Cache-friendly, append-only during extraction
//! - **Relationships stored separately**: Keeps element iteration compact
//! - **Optional container markers**: `ListStart`/`ListEnd` etc. improve tree derivation
//!   when present; depth-based heuristics used as fallback
//! - **OCR elements unified**: OCR text is just another element kind, not a parallel structure
//! - **Blake3 IDs**: Deterministic, collision-resistant identifiers

use std::fmt;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use super::document_structure::{ContentLayer, TextAnnotation};
use super::extraction::BoundingBox;
use super::metadata::Metadata;
use super::ocr_elements::{OcrBoundingGeometry, OcrConfidence, OcrElementLevel, OcrRotation};
use super::tables::Table;
use crate::types::ExtractedImage;

const SUPPRESS_IMAGE_OCR_RENDER_ATTRIBUTE: &str = "xberg:internal:suppress-image-ocr-render";

#[cfg_attr(alef, alef(skip))]
/// Deterministic element identifier, generated via blake3 hashing.
///
/// Format: `"ie-{12 hex chars}"` (48 bits from blake3, ~281 trillion address space).
/// Same input always produces the same ID, enabling diffing and caching.
///
/// Serializes as a plain string (`"ie-aabbccddeeff"`).
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InternalElementId([u8; 15]);

impl Serialize for InternalElementId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for InternalElementId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.len() != 15 {
            return Err(serde::de::Error::custom(format!(
                "InternalElementId must be 15 bytes, got {}",
                s.len()
            )));
        }
        let mut buf = [0u8; 15];
        buf.copy_from_slice(s.as_bytes());
        Ok(Self(buf))
    }
}

impl InternalElementId {
    /// Generate a deterministic ID from element content.
    ///
    /// Hashes the element kind discriminant, text content, page number, and
    /// positional index using blake3. Takes 48 bits (6 bytes) of the hash.
    pub(crate) fn generate(kind_discriminant: &str, text: &str, page: Option<u32>, index: u32) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(kind_discriminant.as_bytes());
        hasher.update(text.as_bytes());
        hasher.update(&page.unwrap_or(u32::MAX).to_le_bytes());
        hasher.update(&index.to_le_bytes());
        let hash = hasher.finalize();
        let bytes = &hash.as_bytes()[..6];
        let mut buf = [0u8; 15];
        buf[0] = b'i';
        buf[1] = b'e';
        buf[2] = b'-';
        hex::encode_to_slice(bytes, &mut buf[3..]).expect("fixed size");
        Self(buf)
    }

    /// Create from a pre-computed ID string.
    ///
    /// The input must be exactly 15 bytes in `"ie-{12 hex}"` format.
    /// Panics if the input length is not 15.
    #[allow(dead_code)]
    pub fn new(id: &str) -> Self {
        assert!(
            id.len() == 15,
            "InternalElementId must be exactly 15 bytes, got {}",
            id.len()
        );
        let mut buf = [0u8; 15];
        buf.copy_from_slice(id.as_bytes());
        Self(buf)
    }

    /// Get the ID as a string slice.
    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap()
    }
}

impl fmt::Display for InternalElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for InternalElementId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg_attr(alef, alef(skip))]
/// The internal flat document representation.
///
/// All extractors output this structure. It is converted to the public
/// [`ExtractedDocument`](super::extraction::ExtractedDocument) and
/// [`DocumentStructure`](super::document_structure::DocumentStructure) in the pipeline.
///
/// Implements `Serialize`/`Deserialize` so that foreign-language plugin implementations
/// (Python, TypeScript, Ruby, etc.) can construct and return this type via JSON at the
/// FFI/trait-bridge boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InternalDocument {
    /// All elements in reading order. Append-only during extraction.
    pub elements: Vec<InternalElement>,

    /// Relationships between elements (source index → target).
    /// Stored separately from elements for cache-friendly iteration.
    pub relationships: Vec<Relationship>,

    /// Source format identifier (e.g., "pdf", "docx", "html", "markdown").
    pub source_format: String,

    /// Document-level metadata (title, author, dates, etc.).
    pub metadata: Metadata,

    /// Extracted images (binary data). Referenced by index from `ElementKind::Image`.
    pub images: Vec<ExtractedImage>,

    /// Extracted tables (structured data). Referenced by index from `ElementKind::Table`.
    pub tables: Vec<Table>,

    /// URIs/links discovered during extraction (hyperlinks, image refs, citations, etc.).
    pub uris: Vec<super::uri::ExtractedUri>,

    /// Archive children: fully-extracted results for files within an archive.
    ///
    /// Only populated by archive extractors (ZIP, TAR, 7z, GZIP) when recursive
    /// extraction is enabled. Each entry contains the full `ExtractedDocument` for
    /// a child file that was extracted through the public pipeline.
    pub children: Option<Vec<crate::types::ArchiveEntry>>,

    /// MIME type of the source document (e.g., "application/pdf", "text/html").
    pub mime_type: String,

    /// Non-fatal warnings collected during extraction.
    pub processing_warnings: Vec<crate::types::ProcessingWarning>,

    /// PDF annotations (links, highlights, notes).
    pub annotations: Option<Vec<crate::types::annotations::PdfAnnotation>>,

    /// Pre-built per-page content (set by extractors that track page boundaries natively).
    ///
    /// When populated, `derive_extraction_result` uses this directly instead of
    /// attempting to reconstruct pages from element-level page numbers.
    pub prebuilt_pages: Option<Vec<crate::types::PageContent>>,

    /// Pre-rendered formatted content produced by the extractor itself.
    ///
    /// When an extractor has direct access to high-quality formatted output (e.g.,
    /// html-to-markdown produces GFM markdown), it can store that here to bypass
    /// the lossy InternalDocument → renderer round-trip. `derive_extraction_result`
    /// will use this directly when the requested output format matches
    /// `metadata.output_format`.
    pub pre_rendered_content: Option<String>,

    /// Pre-built OCR element list (set by extractors that have direct access to
    /// bounding-box element data alongside a separately produced coherent text).
    ///
    /// When populated, `derive_extraction_result` uses this directly instead of
    /// reconstructing `OcrElement`s from `OcrText` `InternalElement`s. This lets
    /// the image extractor carry Tesseract/paddle-ocr bounding-box metadata without
    /// injecting raw word tokens into the element list (which would otherwise corrupt
    /// `render_plain` and page content — issue #706).
    pub prebuilt_ocr_elements: Option<Vec<crate::types::ocr_elements::OcrElement>>,

    /// LLM usage records accumulated during extraction (e.g., VLM OCR per page).
    ///
    /// Populated by extractors that call LLM-backed backends (VLM OCR).
    /// `derive_extraction_result` transfers this to `ExtractedDocument.llm_usage`.
    pub llm_usage: Option<Vec<crate::types::LlmUsage>>,

    /// Track-changes revisions embedded in the source document.
    ///
    /// Set by format-specific extractors (DOCX, ODT, …) that parse
    /// change-tracking markup. `derive_extraction_result` transfers this
    /// directly to `ExtractedDocument.revisions`.
    pub revisions: Option<Vec<crate::types::revisions::DocumentRevision>>,

    /// PDF form fields extracted from AcroForm or XFA-based forms.
    ///
    /// Set by the PDF extractor when `pdf_options.extract_form_fields = true`.
    /// `derive_extraction_result` transfers this directly to `ExtractedDocument.form_fields`.
    pub form_fields: Vec<crate::types::PdfFormField>,

    /// Mathematical formulas recognized during layout-guided OCR.
    ///
    /// Set by the OCR pipeline (per-page formulas, renumbered to document pages).
    /// `derive_extraction_result` transfers this directly to `ExtractedDocument.formulas`.
    pub formulas: Vec<crate::types::Formula>,

    /// When `true`, image OCR results are rendered as plain text without the
    /// `![...](...)` markdown placeholder. Set by the pipeline from
    /// `ImageExtractionConfig.ocr_text_only`.
    #[serde(skip)]
    pub ocr_text_only: bool,

    /// When `true` and `ocr_text_only` is `false`, append the OCR text after
    /// the image placeholder in the rendered output. Set by the pipeline from
    /// `ImageExtractionConfig.append_ocr_text`.
    #[serde(skip)]
    pub append_ocr_text: bool,

    /// When `true` (the default), Markdown rendering backslash-escapes
    /// CommonMark-significant characters (`_[]()*=-#`) so the output round-trips
    /// safely through a CommonMark parser. When `false`, those escapes are
    /// stripped so prose reads identically to the already-unescaped text used in
    /// table cells. Set by the pipeline from `ExtractionConfig::escape_markdown`.
    #[serde(skip)]
    pub escape_markdown: bool,

    /// Page marker format (with `{page_num}` placeholder) when
    /// `PageConfig::insert_page_markers` is enabled, `None` otherwise. Set by
    /// the pipeline. Renderers use it to emit page markers verbatim instead of
    /// escaping or stripping them.
    #[serde(skip)]
    pub page_marker_format: Option<String>,

    /// When `true`, Markdown rendering inserts a `[TABLE:{table_id}]` marker
    /// immediately before each table's rendered Markdown block. Set by the
    /// pipeline from `ExtractionConfig::table_anchors`. Defaults to `false`.
    #[serde(skip)]
    pub table_anchors: bool,
}

impl From<crate::types::extraction::ExtractedDocument> for InternalDocument {
    /// Lossy conversion used at FFI/trait-bridge boundaries where a foreign-language
    /// plugin returns the public `ExtractedDocument` shape but the canonical Rust trait
    /// signature requires an `InternalDocument`. The text content is stashed in
    /// `pre_rendered_content` so the pipeline returns it verbatim instead of trying
    /// to re-render from a non-existent element tree.
    fn from(result: crate::types::extraction::ExtractedDocument) -> Self {
        let mut doc = Self::new(result.mime_type.as_ref());
        doc.mime_type = result.mime_type.into_owned();
        doc.metadata = result.metadata;
        doc.tables = result.tables;
        doc.images = result.images.unwrap_or_default();
        doc.revisions = result.revisions;
        doc.form_fields = result.form_fields;
        doc.formulas = result.formulas;
        doc.pre_rendered_content = if result.content.is_empty() {
            None
        } else {
            Some(result.content)
        };
        doc
    }
}

impl From<InternalDocument> for crate::types::extraction::ExtractedDocument {
    /// Run the canonical derivation pipeline with `OutputFormat::Plain` and no document
    /// structure derivation. Used at FFI/trait-bridge boundaries where the rich
    /// `InternalDocument` must be converted to the public `ExtractedDocument` shape.
    fn from(doc: InternalDocument) -> Self {
        crate::extraction::derive::derive_extraction_result(doc, false, crate::core::config::OutputFormat::Plain)
    }
}

impl InternalDocument {
    /// Create a new empty document with the given source format.
    pub fn new(source_format: impl Into<String>) -> Self {
        Self {
            elements: Vec::new(),
            relationships: Vec::new(),
            source_format: source_format.into(),
            metadata: Metadata::default(),
            images: Vec::new(),
            tables: Vec::new(),
            uris: Vec::new(),
            children: None,
            mime_type: "application/octet-stream".to_string(),
            processing_warnings: Vec::new(),
            annotations: None,
            prebuilt_pages: None,
            pre_rendered_content: None,
            prebuilt_ocr_elements: None,
            llm_usage: None,
            revisions: None,
            ocr_text_only: false,
            append_ocr_text: false,
            escape_markdown: true,
            page_marker_format: None,
            table_anchors: false,
            form_fields: Vec::new(),
            formulas: Vec::new(),
        }
    }

    /// Push an element and return its index.
    pub fn push_element(&mut self, element: InternalElement) -> u32 {
        let idx = self.elements.len() as u32;
        self.elements.push(element);
        idx
    }

    /// Push a relationship.
    pub fn push_relationship(&mut self, relationship: Relationship) {
        self.relationships.push(relationship);
    }

    /// Push a table and return its index (for use in `ElementKind::Table`).
    pub fn push_table(&mut self, table: Table) -> u32 {
        let idx = self.tables.len() as u32;
        self.tables.push(table);
        idx
    }

    /// Push an image and return its index (for use in `ElementKind::Image`).
    pub fn push_image(&mut self, image: ExtractedImage) -> u32 {
        let idx = self.images.len() as u32;
        self.images.push(image);
        idx
    }

    /// Maximum number of URIs to collect per document (DoS prevention).
    const MAX_URIS: usize = 100_000;

    /// Push a URI discovered during extraction.
    /// Silently drops URIs beyond `MAX_URIS` to prevent unbounded memory growth.
    pub fn push_uri(&mut self, uri: super::uri::ExtractedUri) {
        if self.uris.len() < Self::MAX_URIS {
            self.uris.push(uri);
        }
    }

    /// Concatenate all element text into a single string, separated by newlines.
    #[cfg(all(test, any(feature = "html", feature = "hwpx")))]
    pub(crate) fn content(&self) -> String {
        self.elements
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A single element in the internal flat document.
///
/// Elements are appended in reading order during extraction. The `depth` field
/// and optional container markers enable tree reconstruction in the derivation step.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalElement {
    /// Deterministic identifier.
    pub id: InternalElementId,

    /// What kind of content this element represents.
    pub kind: ElementKind,

    /// Primary text content. Empty for non-text elements (images, page breaks).
    pub text: String,

    /// Nesting depth (0 = root level).
    ///
    /// Extractors set this based on heading level, list indent, blockquote depth, etc.
    /// The tree derivation step uses depth changes to reconstruct parent-child relationships.
    pub depth: u16,

    /// Page number (1-indexed). `None` for non-paginated formats.
    pub page: Option<u32>,

    /// Bounding box in document coordinates.
    pub bbox: Option<BoundingBox>,

    /// Content layer classification (Body, Header, Footer, Footnote).
    pub layer: ContentLayer,

    /// Inline annotations (formatting, links) on this element's text content.
    /// Byte-range based, reuses the existing `TextAnnotation` type.
    pub annotations: Vec<TextAnnotation>,

    /// Format-specific key-value attributes.
    /// Used for CSS classes, LaTeX env names, slide layout names, etc.
    pub attributes: Option<AHashMap<String, String>>,

    /// Optional anchor/key for this element.
    ///
    /// Used by the relationship resolver to match references to targets.
    /// Examples: heading slug `"introduction"`, footnote label `"fn1"`,
    /// citation key `"smith2024"`, figure label `"fig:diagram"`.
    pub anchor: Option<String>,

    /// OCR bounding geometry (rectangle or quadrilateral).
    pub ocr_geometry: Option<OcrBoundingGeometry>,

    /// OCR confidence scores (detection + recognition).
    pub ocr_confidence: Option<OcrConfidence>,

    /// OCR rotation metadata.
    pub ocr_rotation: Option<OcrRotation>,
}

impl InternalElement {
    /// Create a simple text element with minimal fields.
    pub fn text(kind: ElementKind, text: impl Into<String>, depth: u16) -> Self {
        let text = text.into();
        let id = InternalElementId::generate(kind.discriminant(), &text, None, 0);
        Self {
            id,
            kind,
            text,
            depth,
            page: None,
            bbox: None,
            layer: ContentLayer::Body,
            annotations: Vec::new(),
            attributes: None,
            anchor: None,
            ocr_geometry: None,
            ocr_confidence: None,
            ocr_rotation: None,
        }
    }

    /// Set the page number.
    #[cfg(any(
        feature = "ocr",
        feature = "office",
        feature = "pdf",
        feature = "paddle-ocr",
        feature = "xml",
        feature = "hwpx",
        feature = "quality",
        feature = "chunking"
    ))]
    #[allow(dead_code)]
    pub(crate) fn with_page(mut self, page: u32) -> Self {
        self.page = Some(page);
        self
    }

    /// Set the bounding box.
    #[cfg(feature = "office")]
    pub(crate) fn with_bbox(mut self, bbox: BoundingBox) -> Self {
        self.bbox = Some(bbox);
        self
    }

    /// Set the content layer.
    #[cfg(all(
        test,
        any(
            feature = "ocr",
            feature = "pdf",
            feature = "paddle-ocr",
            feature = "xml",
            feature = "office"
        )
    ))]
    pub(crate) fn with_layer(mut self, layer: ContentLayer) -> Self {
        self.layer = layer;
        self
    }

    /// Set the anchor key.
    #[cfg(test)]
    pub(crate) fn with_anchor(mut self, anchor: impl Into<String>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }

    /// Set attributes.
    #[cfg(any(feature = "xml", feature = "hwpx"))]
    pub(crate) fn with_attributes(mut self, attributes: AHashMap<String, String>) -> Self {
        self.attributes = Some(attributes);
        self
    }

    /// Regenerate the ID with the correct index (call after pushing to the document).
    #[cfg(any(
        feature = "ocr",
        feature = "xml",
        feature = "archives",
        feature = "hwpx",
        // The only bare-`ocr-pipeline` caller lives in `extractors::pdf::ocr`, so gate on
        // pdf+ocr-pipeline. `ocr-wasm` enables ocr-pipeline without pdf and has no caller. ~keep
        all(feature = "pdf", feature = "ocr-pipeline")
    ))]
    pub(crate) fn with_index(mut self, index: u32) -> Self {
        self.id = InternalElementId::generate(self.kind.discriminant(), &self.text, self.page, index);
        self
    }

    /// Mark an image element so whole-page OCR can replace its nested OCR text
    /// without removing the image placeholder or mutating the public image data.
    ///
    /// Only called by the PDF OCR merge planner (`extractors::pdf::ocr`); dead in
    /// builds that enable `ocr`/`ocr-pipeline` without `pdf`. ~keep
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    pub(crate) fn suppress_image_ocr_rendering(&mut self) {
        self.attributes
            .get_or_insert_with(AHashMap::new)
            .insert(SUPPRESS_IMAGE_OCR_RENDER_ATTRIBUTE.to_string(), "true".to_string());
    }

    /// Whether renderers should include nested OCR text for this image element.
    pub(crate) fn should_render_image_ocr(&self) -> bool {
        !self
            .attributes
            .as_ref()
            .is_some_and(|attributes| attributes.contains_key(SUPPRESS_IMAGE_OCR_RENDER_ATTRIBUTE))
    }

    /// Attributes safe to expose through the public document structure.
    pub(crate) fn public_attributes(&self) -> Option<std::collections::HashMap<String, String>> {
        let original = self.attributes.as_ref()?;
        let attributes: std::collections::HashMap<String, String> = original
            .iter()
            .filter(|(key, _)| key.as_str() != SUPPRESS_IMAGE_OCR_RENDER_ATTRIBUTE)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if attributes.is_empty() && !original.is_empty() {
            None
        } else {
            Some(attributes)
        }
    }
}

/// Semantic role of an internal element.
///
/// Superset of [`NodeContent`](super::document_structure::NodeContent) variants
/// plus OCR and container markers.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElementKind {
    /// Document title.
    Title,
    /// Section heading with level (1-6).
    Heading {
        /// Heading depth (1 = h1, 2 = h2, …, 6 = h6).
        level: u8,
    },
    /// Body text paragraph.
    Paragraph,
    /// List item. `ordered` indicates numbered vs bulleted.
    ListItem {
        /// `true` for ordered (numbered) lists; `false` for unordered (bullet) lists.
        ordered: bool,
    },
    /// Code block. Language stored in element attributes.
    Code,
    /// Mathematical formula / equation.
    Formula,
    /// Footnote content (the definition, not the reference marker).
    FootnoteDefinition,
    /// Footnote reference marker in body text.
    FootnoteRef,
    /// Citation or bibliographic reference.
    Citation,
    /// Presentation slide container.
    Slide {
        /// 1-indexed slide number.
        number: u32,
    },
    /// Definition list term.
    DefinitionTerm,
    /// Definition list description.
    DefinitionDescription,
    /// Admonition / callout (note, warning, tip, etc.). Kind stored in attributes.
    Admonition,
    /// Raw block preserved verbatim. Format stored in attributes.
    RawBlock,
    /// Structured metadata block (frontmatter, email headers).
    MetadataBlock,

    /// Start of a list container.
    ListStart {
        /// `true` for ordered (numbered) lists; `false` for unordered (bullet) lists.
        ordered: bool,
    },
    /// End of a list container.
    ListEnd,
    /// Start of a block quote.
    QuoteStart,
    /// End of a block quote.
    QuoteEnd,
    /// Start of a generic group/section.
    GroupStart,
    /// End of a generic group/section.
    GroupEnd,

    /// Table reference. `table_index` is an index into `InternalDocument::tables`.
    Table {
        /// Index into `InternalDocument::tables` for the referenced table.
        table_index: u32,
    },
    /// Image reference. `image_index` is an index into `InternalDocument::images`.
    Image {
        /// Index into `InternalDocument::images` for the referenced image.
        image_index: u32,
    },
    /// Page break marker.
    PageBreak,

    /// OCR-detected text at a given hierarchical level.
    OcrText {
        /// Hierarchical level (word, line, paragraph, block) of this OCR element.
        level: OcrElementLevel,
    },
}

impl ElementKind {
    /// Get a stable string discriminant for ID generation.
    pub(crate) fn discriminant(&self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Heading { .. } => "heading",
            Self::Paragraph => "paragraph",
            Self::ListItem { .. } => "list_item",
            Self::Code => "code",
            Self::Formula => "formula",
            Self::FootnoteDefinition => "footnote_definition",
            Self::FootnoteRef => "footnote_ref",
            Self::Citation => "citation",
            Self::Slide { .. } => "slide",
            Self::DefinitionTerm => "definition_term",
            Self::DefinitionDescription => "definition_description",
            Self::Admonition => "admonition",
            Self::RawBlock => "raw_block",
            Self::MetadataBlock => "metadata_block",
            Self::ListStart { .. } => "list_start",
            Self::ListEnd => "list_end",
            Self::QuoteStart => "quote_start",
            Self::QuoteEnd => "quote_end",
            Self::GroupStart => "group_start",
            Self::GroupEnd => "group_end",
            Self::Table { .. } => "table",
            Self::Image { .. } => "image",
            Self::PageBreak => "page_break",
            Self::OcrText { .. } => "ocr_text",
        }
    }

    /// Returns true if this is a container start marker.
    pub(crate) fn is_container_start(&self) -> bool {
        matches!(self, Self::ListStart { .. } | Self::QuoteStart | Self::GroupStart)
    }

    /// Returns true if this is a container end marker.
    pub(crate) fn is_container_end(&self) -> bool {
        matches!(self, Self::ListEnd | Self::QuoteEnd | Self::GroupEnd)
    }

    /// Returns the matching end marker for a container start, if applicable.
    #[cfg(test)]
    pub(crate) fn matching_end(&self) -> Option<ElementKind> {
        match self {
            Self::ListStart { .. } => Some(Self::ListEnd),
            Self::QuoteStart => Some(Self::QuoteEnd),
            Self::GroupStart => Some(Self::GroupEnd),
            _ => None,
        }
    }
}

/// A relationship between two elements in the document.
///
/// During extraction, targets may be unresolved keys (`RelationshipTarget::Key`).
/// The derivation step resolves these to indices using the element anchor index.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    /// Index of the source element in `InternalDocument::elements`.
    pub source: u32,

    /// Target of the relationship (resolved index or unresolved key).
    pub target: RelationshipTarget,

    /// Semantic kind of the relationship.
    pub kind: RelationshipKind,
}

/// Target of a relationship — either a resolved element index or an unresolved key.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RelationshipTarget {
    /// Resolved: index into `InternalDocument::elements`.
    Index(u32),
    /// Unresolved: key to be matched against element anchors during derivation.
    Key(String),
}

pub use super::document_structure::RelationshipKind;

const _: () = {
    #[allow(dead_code)]
    fn assert_send_sync<T: Send + Sync>() {}
    #[allow(dead_code)]
    fn _check() {
        assert_send_sync::<InternalDocument>();
        assert_send_sync::<InternalElement>();
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_element_id_deterministic() {
        let id1 = InternalElementId::generate("heading", "Introduction", Some(1), 0);
        let id2 = InternalElementId::generate("heading", "Introduction", Some(1), 0);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_internal_element_id_differs_by_index() {
        let id1 = InternalElementId::generate("paragraph", "Same text", Some(1), 0);
        let id2 = InternalElementId::generate("paragraph", "Same text", Some(1), 1);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_internal_element_id_format() {
        let id = InternalElementId::generate("title", "Hello", None, 0);
        assert!(id.as_str().starts_with("ie-"));
        assert_eq!(id.as_str().len(), 3 + 12);
    }

    #[test]
    fn test_element_kind_discriminant() {
        assert_eq!(ElementKind::Title.discriminant(), "title");
        assert_eq!(ElementKind::Heading { level: 2 }.discriminant(), "heading");
        assert_eq!(ElementKind::ListStart { ordered: true }.discriminant(), "list_start");
    }

    #[test]
    fn test_container_markers() {
        assert!(ElementKind::ListStart { ordered: false }.is_container_start());
        assert!(ElementKind::ListEnd.is_container_end());
        assert!(!ElementKind::Paragraph.is_container_start());
        assert_eq!(ElementKind::QuoteStart.matching_end(), Some(ElementKind::QuoteEnd));
    }

    #[test]
    fn test_internal_document_push() {
        let mut doc = InternalDocument::new("markdown");
        let elem = InternalElement::text(ElementKind::Paragraph, "Hello world", 0);
        let idx = doc.push_element(elem);
        assert_eq!(idx, 0);
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].text, "Hello world");
    }

    #[test]
    fn public_attributes_preserve_explicit_empty_map() {
        let mut element = InternalElement::text(ElementKind::Paragraph, "text", 0);
        element.attributes = Some(AHashMap::new());

        assert_eq!(element.public_attributes(), Some(std::collections::HashMap::new()));
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn public_attributes_hide_internal_image_ocr_suppression() {
        let mut element = InternalElement::text(ElementKind::Image { image_index: 0 }, "", 0);
        element.suppress_image_ocr_rendering();

        assert!(element.public_attributes().is_none());
        assert!(!element.should_render_image_ocr());
    }

    #[cfg(any(
        feature = "ocr",
        feature = "pdf",
        feature = "paddle-ocr",
        feature = "xml",
        feature = "office"
    ))]
    #[test]
    fn test_internal_element_builder_pattern() {
        let elem = InternalElement::text(ElementKind::Heading { level: 2 }, "Methods", 1)
            .with_page(3)
            .with_anchor("methods")
            .with_layer(ContentLayer::Body);

        assert_eq!(elem.text, "Methods");
        assert_eq!(elem.page, Some(3));
        assert_eq!(elem.anchor, Some("methods".to_string()));
        assert_eq!(elem.depth, 1);
    }

    #[test]
    fn test_relationship_kind_serde() {
        let kind = RelationshipKind::FootnoteReference;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"footnote_reference\"");

        let parsed: RelationshipKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    /// Verify that `InternalDocument` round-trips through serde JSON without loss.
    ///
    /// This is the primary correctness gate for foreign-language plugin support:
    /// Python/TypeScript/Ruby implementations of `DocumentExtractor` construct
    /// an `InternalDocument` as JSON and pass it across the FFI boundary.
    #[test]
    fn should_round_trip_through_serde_json() {
        let mut doc = InternalDocument::new("pdf");
        doc.mime_type = "application/pdf".to_string();

        let title = InternalElement::text(ElementKind::Title, "Test Document", 0);
        doc.push_element(title);

        let heading = InternalElement::text(ElementKind::Heading { level: 2 }, "Introduction", 1);
        doc.push_element(heading);

        let para = InternalElement::text(ElementKind::Paragraph, "Body text here.", 1);
        doc.push_element(para);

        let list_start = InternalElement::text(ElementKind::ListStart { ordered: true }, "", 1);
        doc.push_element(list_start);
        let item = InternalElement::text(ElementKind::ListItem { ordered: true }, "First item", 2);
        doc.push_element(item);
        let list_end = InternalElement::text(ElementKind::ListEnd, "", 1);
        doc.push_element(list_end);

        let code = InternalElement::text(ElementKind::Code, "fn main() {}", 0);
        doc.push_element(code);

        let pb = InternalElement::text(ElementKind::PageBreak, "", 0);
        doc.push_element(pb);

        let img_elem = InternalElement::text(ElementKind::Image { image_index: 0 }, "", 0);
        doc.push_element(img_elem);

        let ocr = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Word,
            },
            "scanned word",
            0,
        );
        doc.push_element(ocr);

        doc.push_relationship(Relationship {
            source: 0,
            target: RelationshipTarget::Index(2),
            kind: RelationshipKind::FootnoteReference,
        });
        doc.push_relationship(Relationship {
            source: 1,
            target: RelationshipTarget::Key("introduction".to_string()),
            kind: RelationshipKind::CrossReference,
        });

        let json = serde_json::to_string(&doc).expect("serialize InternalDocument");
        let restored: InternalDocument = serde_json::from_str(&json).expect("deserialize InternalDocument");

        assert_eq!(restored.source_format, doc.source_format);
        assert_eq!(restored.mime_type, doc.mime_type);
        assert_eq!(restored.elements.len(), doc.elements.len());
        assert_eq!(restored.relationships.len(), doc.relationships.len());

        assert_eq!(restored.elements[0].kind, ElementKind::Title);
        assert_eq!(restored.elements[1].kind, ElementKind::Heading { level: 2 });
        assert_eq!(restored.elements[4].kind, ElementKind::ListItem { ordered: true });
        assert_eq!(restored.elements[8].kind, ElementKind::Image { image_index: 0 });
        assert_eq!(
            restored.elements[9].kind,
            ElementKind::OcrText {
                level: OcrElementLevel::Word
            }
        );

        assert_eq!(restored.relationships[0].target, RelationshipTarget::Index(2));
        assert_eq!(
            restored.relationships[1].target,
            RelationshipTarget::Key("introduction".to_string())
        );

        assert_eq!(restored.elements[0].id, doc.elements[0].id);

        assert_eq!(restored.elements[0].layer, ContentLayer::Body);
    }

    /// Cover all 27 `ElementKind` variants through a serde JSON round-trip.
    ///
    /// Every variant must be constructed, serialised, and deserialised; the
    /// `kind` field is then asserted on each restored element so that a missing
    /// or mis-tagged variant surfaces immediately.
    #[test]
    fn should_cover_all_element_kind_variants() {
        let mut doc = InternalDocument::new("test");

        doc.push_element(InternalElement::text(ElementKind::Title, "T", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Title);

        doc.push_element(InternalElement::text(ElementKind::Heading { level: 1 }, "H1", 1));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Heading { level: 1 });

        doc.push_element(InternalElement::text(ElementKind::Paragraph, "P", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Paragraph);

        doc.push_element(InternalElement::text(ElementKind::ListItem { ordered: false }, "li", 2));
        assert_eq!(
            doc.elements.last().unwrap().kind,
            ElementKind::ListItem { ordered: false }
        );

        doc.push_element(InternalElement::text(ElementKind::Code, "x=1", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Code);

        doc.push_element(InternalElement::text(ElementKind::Formula, "E=mc^2", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Formula);

        doc.push_element(InternalElement::text(ElementKind::FootnoteDefinition, "note text", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::FootnoteDefinition);

        doc.push_element(InternalElement::text(ElementKind::FootnoteRef, "1", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::FootnoteRef);

        doc.push_element(InternalElement::text(ElementKind::Citation, "Smith 2020", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Citation);

        doc.push_element(InternalElement::text(ElementKind::Slide { number: 3 }, "slide 3", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Slide { number: 3 });

        doc.push_element(InternalElement::text(ElementKind::DefinitionTerm, "term", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::DefinitionTerm);

        doc.push_element(InternalElement::text(ElementKind::DefinitionDescription, "desc", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::DefinitionDescription);

        doc.push_element(InternalElement::text(ElementKind::Admonition, "Note:", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Admonition);

        doc.push_element(InternalElement::text(ElementKind::RawBlock, "<raw/>", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::RawBlock);

        doc.push_element(InternalElement::text(ElementKind::MetadataBlock, "---", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::MetadataBlock);

        doc.push_element(InternalElement::text(ElementKind::ListStart { ordered: true }, "", 0));
        assert_eq!(
            doc.elements.last().unwrap().kind,
            ElementKind::ListStart { ordered: true }
        );

        doc.push_element(InternalElement::text(ElementKind::ListEnd, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::ListEnd);

        doc.push_element(InternalElement::text(ElementKind::QuoteStart, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::QuoteStart);

        doc.push_element(InternalElement::text(ElementKind::QuoteEnd, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::QuoteEnd);

        doc.push_element(InternalElement::text(ElementKind::GroupStart, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::GroupStart);

        doc.push_element(InternalElement::text(ElementKind::GroupEnd, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::GroupEnd);

        doc.push_element(InternalElement::text(ElementKind::Table { table_index: 0 }, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Table { table_index: 0 });

        doc.push_element(InternalElement::text(ElementKind::Image { image_index: 1 }, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::Image { image_index: 1 });

        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        assert_eq!(doc.elements.last().unwrap().kind, ElementKind::PageBreak);

        for level in [
            OcrElementLevel::Word,
            OcrElementLevel::Line,
            OcrElementLevel::Block,
            OcrElementLevel::Page,
        ] {
            doc.push_element(InternalElement::text(ElementKind::OcrText { level }, "ocr", 0));
            assert_eq!(doc.elements.last().unwrap().kind, ElementKind::OcrText { level });
        }

        let json = serde_json::to_string(&doc).expect("serialize all-variant InternalDocument");
        let restored: InternalDocument = serde_json::from_str(&json).expect("deserialize all-variant InternalDocument");

        assert_eq!(restored.elements.len(), doc.elements.len());

        assert_eq!(restored.elements[0].kind, ElementKind::Title);
        assert_eq!(restored.elements[5].kind, ElementKind::Formula);
        assert_eq!(restored.elements[9].kind, ElementKind::Slide { number: 3 });
        assert_eq!(restored.elements[14].kind, ElementKind::MetadataBlock);
        assert_eq!(restored.elements[16].kind, ElementKind::ListEnd);
        assert_eq!(restored.elements[17].kind, ElementKind::QuoteStart);
        assert_eq!(restored.elements[18].kind, ElementKind::QuoteEnd);
        assert_eq!(restored.elements[19].kind, ElementKind::GroupStart);
        assert_eq!(restored.elements[20].kind, ElementKind::GroupEnd);
        assert_eq!(restored.elements[21].kind, ElementKind::Table { table_index: 0 });
        assert_eq!(restored.elements[23].kind, ElementKind::PageBreak);
        assert_eq!(
            restored.elements[24].kind,
            ElementKind::OcrText {
                level: OcrElementLevel::Word
            }
        );
        assert_eq!(
            restored.elements[27].kind,
            ElementKind::OcrText {
                level: OcrElementLevel::Page
            }
        );
    }

    /// Verify that both `RelationshipTarget` variants survive a serde JSON
    /// round-trip when carried inside a `Relationship`.
    #[test]
    fn should_round_trip_relationship_targets() {
        let mut doc = InternalDocument::new("test");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "source", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "target", 0));

        doc.push_relationship(Relationship {
            source: 0,
            target: RelationshipTarget::Index(1),
            kind: RelationshipKind::CrossReference,
        });
        doc.push_relationship(Relationship {
            source: 0,
            target: RelationshipTarget::Key("anchor-abc".to_string()),
            kind: RelationshipKind::FootnoteReference,
        });

        let json = serde_json::to_string(&doc).expect("serialize RelationshipTarget variants");
        let restored: InternalDocument = serde_json::from_str(&json).expect("deserialize RelationshipTarget variants");

        assert_eq!(restored.relationships.len(), 2);
        assert_eq!(restored.relationships[0].target, RelationshipTarget::Index(1));
        assert_eq!(
            restored.relationships[1].target,
            RelationshipTarget::Key("anchor-abc".to_string())
        );
        assert_eq!(restored.relationships[0].kind, RelationshipKind::CrossReference);
        assert_eq!(restored.relationships[1].kind, RelationshipKind::FootnoteReference);
    }
}
