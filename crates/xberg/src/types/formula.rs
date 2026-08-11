//! Mathematical formula extracted from a document.

use serde::{Deserialize, Serialize};

use super::extraction::BoundingBox;

/// A mathematical formula extracted from a document.
///
/// Three kinds of sources populate this type. Layout-guided OCR detects
/// formula regions and recognizes them; those formulas carry a `bbox` and a
/// `page`. VLM OCR recognizes formulas in transcribed text without layout, so
/// its formulas carry no geometry. Markup extraction (DOCX, PPTX, ODT, EPUB,
/// HTML, JATS, LaTeX, Markdown, and related formats) converts embedded math
/// to LaTeX, also without geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct Formula {
    /// LaTeX source of the formula, without surrounding `$$` delimiters.
    ///
    /// Markup converters and formula OCR produce real LaTeX. The native PDF
    /// layout path stores the plain text of a detected formula region, which
    /// keeps the original Unicode math characters instead of LaTeX commands.
    /// To render the formula in Markdown or other formats, wrap it in `$$..$$`.
    pub latex: String,

    /// Bounding box of the formula region on its page. `None` for markup sources.
    ///
    /// OCR sources report coordinates in the pixel space of the rendered page
    /// image at the OCR render DPI (base 150; reduced automatically for very
    /// large pages). Image sources report source-image pixels. These values are
    /// not comparable to PDF point coordinates. The C FFI reports an absent
    /// bbox as a null pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BoundingBox>,

    /// 1-indexed page number the formula appears on. `None` when the source
    /// format has no page concept. The C FFI reports an absent page as `0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}
