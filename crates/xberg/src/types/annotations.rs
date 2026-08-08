//! PDF annotation types.

use super::extraction::BoundingBox;
use serde::{Deserialize, Serialize};

/// Type of PDF annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PdfAnnotationType {
    /// Sticky note / text annotation
    Text,
    /// Highlighted text region
    Highlight,
    /// Hyperlink annotation
    Link,
    /// Rubber stamp annotation
    Stamp,
    /// Underline text markup
    Underline,
    /// Strikeout text markup
    StrikeOut,
    /// Squiggly (wavy) underline text markup
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Squiggly,
    /// Freehand drawing (ink) annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Ink,
    /// Rectangle/box shape annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Square,
    /// Ellipse/oval shape annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Circle,
    /// Closed polygon shape annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Polygon,
    /// Open polyline shape annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    PolyLine,
    /// Line annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Line,
    /// Caret (text-insertion marker) annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Caret,
    /// Embedded file attachment annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    FileAttachment,
    /// Embedded sound annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Sound,
    /// Embedded movie annotation
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    Movie,
    /// Any other annotation type
    Other,
}

/// A PDF annotation extracted from a document page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PdfAnnotation {
    /// The type of annotation.
    pub annotation_type: PdfAnnotationType,
    /// Text content of the annotation (e.g., comment text, link URL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Page number where the annotation appears (1-indexed).
    pub page_number: u32,
    /// Bounding box of the annotation on the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
    /// Author/creator of the annotation (PDF `/T` entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub author: Option<String>,
    /// Last modification date of the annotation (PDF `/M` entry), as a raw
    /// PDF date string (e.g. `"D:20240115120000Z"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub modified: Option<String>,
    /// Annotation colour (PDF `/C` entry), normalised to a CSS-compatible
    /// `#rrggbb` hex string. Gray and CMYK colour spaces are converted to RGB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub color: Option<String>,
    /// Subject of the annotation (PDF `/Subj` entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub subject: Option<String>,
    /// Per-line bounding boxes derived from the annotation's `/QuadPoints`
    /// entry. Present for text markup annotations (Highlight, Underline,
    /// StrikeOut, Squiggly), one box per marked line/run of text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub quad_points: Option<Vec<BoundingBox>>,
    /// The document text covered by [`Self::quad_points`], recovered from the
    /// page content underneath the marked-up region. Populated for
    /// Highlight, Underline, StrikeOut, and Squiggly annotations when the
    /// underlying text could be recovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub marked_text: Option<String>,
}
