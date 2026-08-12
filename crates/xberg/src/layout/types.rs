use serde::{Deserialize, Serialize};
use std::fmt;

/// Bounding box in original image coordinates (x1, y1) top-left, (x2, y2) bottom-right.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BBox {
    /// Left edge (x-coordinate of the top-left corner).
    pub x1: f32,
    /// Top edge (y-coordinate of the top-left corner).
    pub y1: f32,
    /// Right edge (x-coordinate of the bottom-right corner).
    pub x2: f32,
    /// Bottom edge (y-coordinate of the bottom-right corner).
    pub y2: f32,
}

#[allow(dead_code)]
impl BBox {
    pub(crate) fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    pub(crate) fn width(&self) -> f32 {
        (self.x2 - self.x1).max(0.0)
    }

    pub(crate) fn height(&self) -> f32 {
        (self.y2 - self.y1).max(0.0)
    }

    pub(crate) fn area(&self) -> f32 {
        self.width() * self.height()
    }

    /// Area of intersection with another bounding box.
    pub(crate) fn intersection_area(&self, other: &BBox) -> f32 {
        let x1 = self.x1.max(other.x1);
        let y1 = self.y1.max(other.y1);
        let x2 = self.x2.min(other.x2);
        let y2 = self.y2.min(other.y2);
        (x2 - x1).max(0.0) * (y2 - y1).max(0.0)
    }

    /// Intersection over Union with another bounding box.
    pub(crate) fn iou(&self, other: &BBox) -> f32 {
        let inter = self.intersection_area(other);
        let union = self.area() + other.area() - inter;
        if union <= 0.0 { 0.0 } else { inter / union }
    }

    /// Fraction of `other` that is contained within `self`.
    /// Returns 0.0..=1.0 where 1.0 means `other` is fully inside `self`.
    pub(crate) fn containment_of(&self, other: &BBox) -> f32 {
        let other_area = other.area();
        if other_area <= 0.0 {
            return 0.0;
        }
        self.intersection_area(other) / other_area
    }

    /// Fraction of page area this bbox covers.
    pub(crate) fn page_coverage(&self, page_width: f32, page_height: f32) -> f32 {
        let page_area = page_width * page_height;
        if page_area <= 0.0 {
            return 0.0;
        }
        self.area() / page_area
    }
}

impl fmt::Display for BBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:.1}, {:.1}, {:.1}, {:.1}]", self.x1, self.y1, self.x2, self.y2)
    }
}

/// The 18 canonical document layout classes.
///
/// All model backends (RT-DETR, YOLO, etc.) map their native class IDs
/// to this shared set. Models with fewer classes (DocLayNet: 11, PubLayNet: 5)
/// map to the closest equivalent.
///
/// Wire format is snake_case in all serializers (JSON, TOML, YAML).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayoutClass {
    /// Figure or table caption text.
    #[default]
    Caption,
    /// Chart or graph visualization.
    Chart,
    /// Footnote or endnote text.
    Footnote,
    /// Mathematical formula or equation.
    Formula,
    /// A single item in a bulleted or numbered list.
    ListItem,
    /// Running footer at the bottom of a page.
    PageFooter,
    /// Running header at the top of a page.
    PageHeader,
    /// Image, chart, or other graphical element.
    Picture,
    /// Section heading.
    SectionHeader,
    /// Data table.
    Table,
    /// Body text paragraph.
    Text,
    /// Document or chapter title.
    Title,
    /// Table of contents or index.
    DocumentIndex,
    /// Source code block.
    Code,
    /// Checkbox in selected state.
    CheckboxSelected,
    /// Checkbox in unselected state.
    CheckboxUnselected,
    /// Form field or form element.
    Form,
    /// Key-value pair region (e.g. label + value in a form).
    KeyValueRegion,
}

#[allow(dead_code)]
impl LayoutClass {
    /// Map from Docling RT-DETR model label ID (0-16) to LayoutClass.
    pub(crate) fn from_docling_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(Self::Caption),
            1 => Some(Self::Footnote),
            2 => Some(Self::Formula),
            3 => Some(Self::ListItem),
            4 => Some(Self::PageFooter),
            5 => Some(Self::PageHeader),
            6 => Some(Self::Picture),
            7 => Some(Self::SectionHeader),
            8 => Some(Self::Table),
            9 => Some(Self::Text),
            10 => Some(Self::Title),
            11 => Some(Self::DocumentIndex),
            12 => Some(Self::Code),
            13 => Some(Self::CheckboxSelected),
            14 => Some(Self::CheckboxUnselected),
            15 => Some(Self::Form),
            16 => Some(Self::KeyValueRegion),
            _ => None,
        }
    }

    /// Map from DocLayNet class ID (0-10) to LayoutClass.
    ///
    /// DocLayNet classes: Caption, Footnote, Formula, List-item, Page-footer,
    /// Page-header, Picture, Section-header, Table, Text, Title.
    pub(crate) fn from_doclaynet_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(Self::Caption),
            1 => Some(Self::Footnote),
            2 => Some(Self::Formula),
            3 => Some(Self::ListItem),
            4 => Some(Self::PageFooter),
            5 => Some(Self::PageHeader),
            6 => Some(Self::Picture),
            7 => Some(Self::SectionHeader),
            8 => Some(Self::Table),
            9 => Some(Self::Text),
            10 => Some(Self::Title),
            _ => None,
        }
    }

    /// Map from DocStructBench class ID (0-9) to LayoutClass.
    ///
    /// DocStructBench classes: Title, Plain Text, Abandoned Text, Figure,
    /// Figure Caption, Table, Table Caption, Table Footnote, Isolated Formula, Formula Caption.
    pub(crate) fn from_docstructbench_id(id: i64) -> Option<Self> {
        match id {
            0 => Some(Self::Title),
            1 => Some(Self::Text),
            2 => Some(Self::Text),
            3 => Some(Self::Picture),
            4 => Some(Self::Caption),
            5 => Some(Self::Table),
            6 => Some(Self::Caption),
            7 => Some(Self::Footnote),
            8 => Some(Self::Formula),
            9 => Some(Self::Caption),
            _ => None,
        }
    }

    /// Snake_case wire-format name (matches serde output).
    ///
    /// Equivalent to `format!("{self}")` but returns a `&'static str` slice
    /// for callers that need to avoid allocations.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Caption => "caption",
            Self::Chart => "chart",
            Self::Footnote => "footnote",
            Self::Formula => "formula",
            Self::ListItem => "list_item",
            Self::PageFooter => "page_footer",
            Self::PageHeader => "page_header",
            Self::Picture => "picture",
            Self::SectionHeader => "section_header",
            Self::Table => "table",
            Self::Text => "text",
            Self::Title => "title",
            Self::DocumentIndex => "document_index",
            Self::Code => "code",
            Self::CheckboxSelected => "checkbox_selected",
            Self::CheckboxUnselected => "checkbox_unselected",
            Self::Form => "form",
            Self::KeyValueRegion => "key_value_region",
        }
    }
}

impl fmt::Display for LayoutClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl BBox {
    /// Clamp this box to an image of `width` x `height` pixels, returning the
    /// integer crop rectangle `(x, y, w, h)`. `None` when the clamped region
    /// is empty.
    pub(crate) fn clamp_to_image(&self, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
        let x1 = (self.x1.max(0.0) as u32).min(width.saturating_sub(1));
        let y1 = (self.y1.max(0.0) as u32).min(height.saturating_sub(1));
        let x2 = (self.x2.max(0.0).ceil() as u32).min(width);
        let y2 = (self.y2.max(0.0).ceil() as u32).min(height);
        let w = x2.saturating_sub(x1);
        let h = y2.saturating_sub(y1);
        if w == 0 || h == 0 { None } else { Some((x1, y1, w, h)) }
    }
}

/// A single layout detection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutDetection {
    /// Detected layout class (e.g. `Table`, `Text`, `Title`).
    pub class_name: LayoutClass,
    /// Detection confidence score in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Bounding box in image pixel coordinates.
    pub bbox: BBox,
}

impl LayoutDetection {
    /// Sort detections by confidence in descending order.
    #[allow(dead_code)]
    pub(crate) fn sort_by_confidence_desc(mut detections: Vec<LayoutDetection>) -> Vec<LayoutDetection> {
        detections.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        detections
    }

    #[allow(dead_code)]
    pub(crate) fn new(class_name: LayoutClass, confidence: f32, bbox: BBox) -> Self {
        Self {
            class_name,
            confidence,
            bbox,
        }
    }

    /// Deprecated: use the `class_name` field directly.
    #[deprecated(since = "4.10.0", note = "Use `class_name` field instead")]
    pub fn class(&self) -> LayoutClass {
        self.class_name
    }
}

impl fmt::Display for LayoutDetection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:20} conf={:.3}  bbox={}",
            self.class_name.as_str(),
            self.confidence,
            self.bbox
        )
    }
}

/// Pre-computed table markdown for a table detection region.
///
/// Produced by the TATR-based table structure recognizer and surfaced as part of
/// layout-aware OCR results.  The struct lives here (under `layout-types`, pure-Rust)
/// so that consumers who do not enable `layout-detection` (ORT) can still reference
/// the type in their own code.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecognizedTable {
    /// Detection bbox that this table corresponds to (for matching).
    pub detection_bbox: BBox,
    /// Table cells as a 2D vector (rows × columns).
    pub cells: Vec<Vec<String>>,
    /// Rendered markdown table.
    pub markdown: String,
}

/// Page-level detection result containing all detections and page metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Page width in pixels (as seen by the model).
    pub page_width: u32,
    /// Page height in pixels (as seen by the model).
    pub page_height: u32,
    /// All layout detections on this page after postprocessing.
    pub detections: Vec<LayoutDetection>,
}

impl DetectionResult {
    #[allow(dead_code)]
    pub(crate) fn new(page_width: u32, page_height: u32, detections: Vec<LayoutDetection>) -> Self {
        Self {
            page_width,
            page_height,
            detections,
        }
    }
}
