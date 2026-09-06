//! Page structure types for documents.
//!
//! This module defines types for representing paginated document structures.

use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;

use super::extraction::BoundingBox;
use super::serde_helpers::serde_vec_arc;
use super::tables::Table;

/// Unified page structure for documents.
///
/// Supports different page types (PDF pages, PPTX slides, Excel sheets)
/// with character offset boundaries for chunk-to-page mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageStructure {
    /// Total number of pages/slides/sheets
    pub total_count: u32,

    /// Type of paginated unit
    pub unit_type: PageUnitType,

    /// Character offset boundaries for each page
    ///
    /// Maps character ranges in the extracted content to page numbers.
    /// Used for chunk page range calculation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundaries: Option<Vec<PageBoundary>>,

    /// Detailed per-page metadata (optional, only when needed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<PageInfo>>,
}

/// Type of paginated unit in a document.
///
/// Distinguishes between different types of "pages" (PDF pages, presentation slides, spreadsheet sheets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub enum PageUnitType {
    /// Standard document pages (PDF, DOCX, images)
    Page,
    /// Presentation slides (PPTX, ODP)
    Slide,
    /// Spreadsheet sheets (XLSX, ODS)
    Sheet,
}

/// Byte offset boundary for a page.
///
/// Tracks where a specific page's content starts and ends in the main content string,
/// enabling mapping from byte positions to page numbers. Offsets are guaranteed to be
/// at valid UTF-8 character boundaries when using standard String methods (push_str, push, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageBoundary {
    /// Byte offset where this page starts in the content string (UTF-8 valid boundary, inclusive)
    pub byte_start: usize,
    /// Byte offset where this page ends in the content string (UTF-8 valid boundary, exclusive)
    pub byte_end: usize,
    /// Page number (1-indexed)
    pub page_number: u32,
}

/// Metadata for individual page/slide/sheet.
///
/// Captures per-page information including dimensions, content counts,
/// and visibility state (for presentations).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct PageDimensions {
    /// Page width in points or pixels.
    pub width: f64,
    /// Page height in points or pixels.
    pub height: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PageDimensionsWire {
    Positional((f64, f64)),
    Named { width: f64, height: f64 },
}

// ~keep Deserialize stays hand-written (not derived) so a legacy positional `[width, height]`
// array from pre-migration callers still parses; only Serialize now emits the named object.
impl<'de> Deserialize<'de> for PageDimensions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match PageDimensionsWire::deserialize(deserializer)? {
            PageDimensionsWire::Positional(dimensions) => dimensions.into(),
            PageDimensionsWire::Named { width, height } => Self { width, height },
        })
    }
}

#[cfg(feature = "api")]
impl utoipa::PartialSchema for PageDimensions {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{Object, ObjectBuilder, Type};

        ObjectBuilder::new()
            .property("width", Object::with_type(Type::Number))
            .required("width")
            .property("height", Object::with_type(Type::Number))
            .required("height")
            .into()
    }
}

#[cfg(feature = "api")]
impl utoipa::ToSchema for PageDimensions {}

impl From<(f64, f64)> for PageDimensions {
    fn from((width, height): (f64, f64)) -> Self {
        Self { width, height }
    }
}

impl From<PageDimensions> for (f64, f64) {
    fn from(dimensions: PageDimensions) -> Self {
        (dimensions.width, dimensions.height)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageInfo {
    /// Page number (1-indexed)
    pub number: u32,

    /// Page title (usually for presentations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Dimensions in points (PDF) or pixels (images).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<PageDimensions>,

    /// Number of images on this page
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_count: Option<u32>,

    /// Number of tables on this page
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_count: Option<u32>,

    /// Whether this page is hidden (e.g., in presentations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,

    /// Whether this page is blank (no meaningful text, no images, no tables)
    ///
    /// A page is considered blank if it has fewer than 3 non-whitespace characters
    /// and contains no tables or images. This is useful for filtering out empty pages
    /// in scanned documents or PDFs with blank separator pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_blank: Option<bool>,

    /// Whether this page contains non-trivial vector graphics (paths, shapes, curves)
    ///
    /// Indicates the presence of vector-drawn content such as charts, diagrams,
    /// or geometric shapes (e.g., from Adobe InDesign, LaTeX TikZ). These are
    /// invisible to `ExtractedDocument.images` since they are not embedded as raster
    /// XObjects. Set to `true` when path count exceeds a heuristic threshold,
    /// signaling that downstream consumers may want to rasterize the page to
    /// capture this content.
    ///
    /// Only populated for PDFs; `None` for other document types.
    #[serde(default, skip_serializing_if = "is_default_bool")]
    pub has_vector_graphics: bool,
}

/// Content for a single page/slide.
///
/// When page extraction is enabled, documents are split into per-page content
/// with associated tables and images mapped to each page.
///
/// # Performance
///
/// Uses Arc-wrapped tables and images for memory efficiency:
/// - `Vec<Arc<Table>>` enables zero-copy sharing of table data
/// - `Vec<Arc<ExtractedImage>>` enables zero-copy sharing of image data
/// - Maintains exact JSON compatibility via custom Serialize/Deserialize
///
/// This reduces memory overhead for documents with shared tables/images
/// by avoiding redundant copies during serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageContent {
    /// Page number (1-indexed)
    pub page_number: u32,

    /// Text content for this page
    pub content: String,

    /// Tables found on this page (uses Arc for memory efficiency)
    ///
    /// Serializes as `Vec<Table>` for JSON compatibility while maintaining
    /// Arc semantics in-memory for zero-copy sharing.
    #[serde(skip_serializing_if = "Vec::is_empty", default, with = "serde_vec_arc")]
    #[cfg_attr(feature = "api", schema(value_type = Vec<Table>))]
    pub tables: Vec<Arc<Table>>,

    /// Indices into `ExtractedDocument.images` for images found on this page.
    ///
    /// Each value is a zero-based index into the top-level `images` collection.
    /// Only populated when `extract_images = true` in the extraction config.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub image_indices: Vec<u32>,

    /// OCR image preprocessing applied to this page's raster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_preprocessing: Option<super::ImagePreprocessingMetadata>,

    /// Hierarchy information for the page (when hierarchy extraction is enabled)
    ///
    /// Contains text hierarchy levels (H1-H6) extracted from the page content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<PageHierarchy>,

    /// Whether this page is blank (no meaningful text content)
    ///
    /// Determined during extraction based on text content analysis.
    /// A page is blank if it has fewer than 3 non-whitespace characters
    /// and contains no tables or images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_blank: Option<bool>,

    /// Layout detection regions for this page (when layout detection is enabled).
    ///
    /// Contains detected layout regions with class, confidence, bounding box,
    /// and area fraction. Only populated when layout detection is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_regions: Option<Vec<LayoutRegion>>,

    /// Speaker notes for this slide (PPTX only).
    ///
    /// Contains the text from the slide's notes pane (`ppt/notesSlides/notesSlide{N}.xml`).
    /// Only populated when the source is a PPTX file and notes are present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_notes: Option<String>,

    /// Section name this slide belongs to (PPTX only).
    ///
    /// PowerPoint sections group slides into logical chapters (`<p:sectionLst>` in
    /// `ppt/presentation.xml`). Only populated when the source is a PPTX file and
    /// the slide belongs to a named section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,

    /// Sheet name for this page (XLSX/ODS only).
    ///
    /// Each spreadsheet sheet maps to one `PageContent` entry. This field carries the
    /// sheet's display name as it appears in the workbook. `None` for all non-spreadsheet
    /// formats and for sheets with an empty name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,

    /// Aggregate OCR confidence for this page. `None` when the page was not OCR'd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_confidence: Option<PageOcrConfidence>,
}

/// Aggregate OCR legibility score for a page, reported by the backend that produced its text.
///
/// This is distinct from [`OcrConfidence`], which scores a single detected element (a word or
/// line) using detection/recognition confidence from the OCR engine itself. `PageOcrConfidence`
/// is a page-level summary computed after noise filtering, intended for triage of which pages
/// are worth a closer look, not for comparing OCR engines against each other.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageOcrConfidence {
    /// Aggregate legibility score in `0.0..=1.0`, or `None` when the backend that
    /// produced this page does not report a calibrated legibility scale.
    ///
    /// Backends differ in what their confidence numbers mean (see `OcrConfidence`'s
    /// per-backend normalization), and not every backend maps onto a 0.0-1.0 legibility
    /// scale at all. When a backend has no such calibrated scale, this is `None` rather
    /// than a misleading number, and scores must never be compared across backends.
    pub score: Option<f64>,

    /// Number of words the score was averaged over, AFTER noise filtering.
    ///
    /// A small `word_count` means the average is based on little evidence, so a high
    /// `score` next to a small `word_count` is not representative of the whole page.
    pub word_count: u32,

    /// Name of the OCR backend that produced the page text.
    pub backend: String,
}

/// A detected layout region on a page.
///
/// When layout detection is enabled, each page may have layout regions
/// identifying different content types (text, pictures, tables, etc.)
/// with confidence scores and spatial positions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LayoutRegion {
    /// Layout class name (e.g. "picture", "table", "text", "section_header").
    #[serde(alias = "class")]
    pub class_name: String,
    /// Confidence score from the layout detection model (0.0 to 1.0).
    pub confidence: f64,
    /// Bounding box in document coordinate space.
    pub bounding_box: BoundingBox,
    /// Fraction of the page area covered by this region (0.0 to 1.0).
    pub area_fraction: f64,
}

impl LayoutRegion {
    /// Deprecated: use the `class_name` field directly.
    #[deprecated(since = "1.1.0", note = "Use `class_name` field instead")]
    pub fn class(&self) -> &str {
        &self.class_name
    }
}

/// Page hierarchy structure containing heading levels and block information.
///
/// Used when PDF text hierarchy extraction is enabled. Contains hierarchical
/// blocks with heading levels (H1-H6) for semantic document structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PageHierarchy {
    /// Number of hierarchy blocks on this page
    pub block_count: u32,

    /// Hierarchical blocks with heading levels
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blocks: Vec<HierarchicalBlock>,
}

/// A text block with hierarchy level assignment.
///
/// Represents a block of text with semantic heading information extracted from
/// font size clustering and hierarchical analysis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct HierarchicalBoundingBox {
    /// Left coordinate.
    pub left: f32,
    /// Top coordinate.
    pub top: f32,
    /// Right coordinate.
    pub right: f32,
    /// Bottom coordinate.
    pub bottom: f32,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HierarchicalBoundingBoxWire {
    Positional((f32, f32, f32, f32)),
    Named {
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
    },
}

// ~keep Deserialize stays hand-written (not derived) so a legacy positional
// `[left, top, right, bottom]` array from pre-migration callers still parses; only Serialize
// now emits the named object.
impl<'de> Deserialize<'de> for HierarchicalBoundingBox {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match HierarchicalBoundingBoxWire::deserialize(deserializer)? {
            HierarchicalBoundingBoxWire::Positional(bbox) => bbox.into(),
            HierarchicalBoundingBoxWire::Named {
                left,
                top,
                right,
                bottom,
            } => Self {
                left,
                top,
                right,
                bottom,
            },
        })
    }
}

#[cfg(feature = "api")]
impl utoipa::PartialSchema for HierarchicalBoundingBox {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{Object, ObjectBuilder, Type};

        ObjectBuilder::new()
            .property("left", Object::with_type(Type::Number))
            .required("left")
            .property("top", Object::with_type(Type::Number))
            .required("top")
            .property("right", Object::with_type(Type::Number))
            .required("right")
            .property("bottom", Object::with_type(Type::Number))
            .required("bottom")
            .into()
    }
}

#[cfg(feature = "api")]
impl utoipa::ToSchema for HierarchicalBoundingBox {}

impl From<(f32, f32, f32, f32)> for HierarchicalBoundingBox {
    fn from((left, top, right, bottom): (f32, f32, f32, f32)) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

impl From<HierarchicalBoundingBox> for (f32, f32, f32, f32) {
    fn from(bbox: HierarchicalBoundingBox) -> Self {
        (bbox.left, bbox.top, bbox.right, bbox.bottom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct HierarchicalBlock {
    /// The text content of this block
    pub text: String,

    /// The font size of the text in this block
    pub font_size: f32,

    /// The hierarchy level of this block (H1-H6 or Body)
    ///
    /// Levels correspond to HTML heading tags:
    /// - "h1": Top-level heading
    /// - "h2": Secondary heading
    /// - "h3": Tertiary heading
    /// - "h4": Quaternary heading
    /// - "h5": Quinary heading
    /// - "h6": Senary heading
    /// - "body": Body text (no heading level)
    pub level: String,

    /// Bounding box information for the block
    ///
    /// Contains left, top, right, and bottom coordinates in PDF units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<HierarchicalBoundingBox>,
}

/// Helper for skipping default bool values in serialization.
fn is_default_bool(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
mod binding_value_serde_tests {
    use super::{HierarchicalBlock, HierarchicalBoundingBox, PageContent, PageDimensions, PageInfo, PageOcrConfidence};
    use serde_json::json;

    #[cfg(feature = "api")]
    fn assert_named_object_schema<T: utoipa::PartialSchema>(fields: &[&str]) {
        let schema = serde_json::to_value(T::schema()).expect("schema must serialize");
        assert_eq!(schema["type"], "object");
        let properties = schema["properties"].as_object().expect("schema must have properties");
        assert_eq!(properties.len(), fields.len());
        for field in fields {
            assert_eq!(properties[*field]["type"], "number");
        }
        let required = schema["required"].as_array().expect("schema must have required fields");
        assert_eq!(required.len(), fields.len());
        for field in fields {
            assert!(required.contains(&json!(field)));
        }
    }

    #[cfg(feature = "api")]
    #[test]
    fn should_describe_binding_dtos_as_named_object_schemas() {
        assert_named_object_schema::<PageDimensions>(&["width", "height"]);
        assert_named_object_schema::<HierarchicalBoundingBox>(&["left", "top", "right", "bottom"]);
    }

    #[test]
    fn should_still_accept_legacy_positional_array_for_page_dimensions() {
        let legacy = json!({
            "number": 1,
            "dimensions": [612.0, 792.0]
        });
        let page: PageInfo = serde_json::from_value(legacy).expect("legacy page info must deserialize");
        let dimensions = page.dimensions.expect("dimensions must be present");

        assert_eq!(dimensions.width, 612.0);
        assert_eq!(dimensions.height, 792.0);
    }

    #[test]
    fn should_serialize_page_dimensions_as_named_object() {
        let named: PageDimensions = serde_json::from_value(json!({"width": 612.0, "height": 792.0}))
            .expect("named page dimensions must deserialize");

        assert_eq!(
            named,
            PageDimensions {
                width: 612.0,
                height: 792.0
            }
        );
        assert_eq!(
            serde_json::to_value(named).expect("named page dimensions must serialize"),
            json!({"width": 612.0, "height": 792.0})
        );

        let page = PageInfo {
            number: 1,
            title: None,
            dimensions: Some(named),
            image_count: None,
            table_count: None,
            hidden: None,
            is_blank: None,
            has_vector_graphics: false,
        };
        assert_eq!(
            serde_json::to_value(page).expect("page info must serialize"),
            json!({"number": 1, "dimensions": {"width": 612.0, "height": 792.0}})
        );
    }

    #[test]
    fn should_accept_page_info_without_dimensions() {
        let page: PageInfo = serde_json::from_value(json!({
            "number": 2
        }))
        .expect("page info without dimensions must deserialize");

        assert!(page.dimensions.is_none());
    }

    #[test]
    fn should_still_accept_legacy_positional_array_for_hierarchical_bounding_box() {
        let legacy = json!({
            "text": "Heading",
            "font_size": 18.0,
            "level": "h1",
            "bbox": [1.0, 2.0, 101.0, 22.0]
        });
        let block: HierarchicalBlock = serde_json::from_value(legacy).expect("legacy hierarchy block must deserialize");
        let bbox = block.bbox.expect("bounding box must be present");

        assert_eq!((bbox.left, bbox.top, bbox.right, bbox.bottom), (1.0, 2.0, 101.0, 22.0));
    }

    #[test]
    fn should_serialize_hierarchical_bounding_box_as_named_object() {
        let named: HierarchicalBoundingBox = serde_json::from_value(json!({
            "left": 1.0,
            "top": 2.0,
            "right": 101.0,
            "bottom": 22.0
        }))
        .expect("named hierarchy bounding box must deserialize");

        assert_eq!(
            named,
            HierarchicalBoundingBox {
                left: 1.0,
                top: 2.0,
                right: 101.0,
                bottom: 22.0
            }
        );
        assert_eq!(
            serde_json::to_value(named).expect("named hierarchy bounding box must serialize"),
            json!({"left": 1.0, "top": 2.0, "right": 101.0, "bottom": 22.0})
        );

        let block = HierarchicalBlock {
            text: "Heading".to_string(),
            font_size: 18.0,
            level: "h1".to_string(),
            bbox: Some(named),
        };
        assert_eq!(
            serde_json::to_value(block).expect("hierarchy block must serialize"),
            json!({
                "text": "Heading",
                "font_size": 18.0,
                "level": "h1",
                "bbox": {"left": 1.0, "top": 2.0, "right": 101.0, "bottom": 22.0}
            })
        );
    }

    fn page_content_without_ocr_confidence() -> PageContent {
        PageContent {
            page_number: 1,
            content: "hello".to_string(),
            tables: Vec::new(),
            image_indices: Vec::new(),
            image_preprocessing: None,
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
            ocr_confidence: None,
        }
    }

    #[test]
    fn should_omit_ocr_confidence_key_when_page_was_not_ocrd() {
        let page = page_content_without_ocr_confidence();

        let value = serde_json::to_value(page).expect("page content must serialize");

        assert!(
            value
                .as_object()
                .expect("page content must serialize as an object")
                .get("ocr_confidence")
                .is_none(),
            "ocr_confidence must be omitted entirely, not serialized as null"
        );
    }

    #[test]
    fn should_deserialize_legacy_page_content_missing_ocr_confidence_field() {
        let legacy = json!({
            "page_number": 1,
            "content": "hello",
        });

        let page: PageContent =
            serde_json::from_value(legacy).expect("page content without ocr_confidence must deserialize");

        assert_eq!(page.ocr_confidence, None);
    }

    #[test]
    fn should_round_trip_ocr_confidence_when_present() {
        let mut page = page_content_without_ocr_confidence();
        page.ocr_confidence = Some(PageOcrConfidence {
            score: Some(0.87),
            word_count: 42,
            backend: "tesseract".to_string(),
        });

        let value = serde_json::to_value(page).expect("page content with ocr_confidence must serialize");

        assert_eq!(
            value["ocr_confidence"],
            json!({"score": 0.87, "word_count": 42, "backend": "tesseract"})
        );
    }
}
