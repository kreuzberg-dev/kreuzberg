//! Mathematical formula extracted from a document.

use serde::{Deserialize, Serialize};

use super::extraction::BoundingBox;

/// A mathematical formula extracted from a document.
///
/// Two kinds of sources populate this type. Layout-guided OCR detects formula
/// regions and recognizes them; those formulas carry a `bbox` and a `page`.
/// Markup extraction (DOCX, PPTX, ODT, EPUB, HTML, JATS, LaTeX, Markdown, and
/// related formats) converts embedded math to LaTeX; those formulas carry no
/// geometry, so `bbox` and `page` are `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct Formula {
    /// LaTeX source of the formula, without surrounding `$$` delimiters.
    ///
    /// To render the formula in Markdown or other formats, wrap it in `$$..$$`.
    pub latex: String,

    /// Bounding box of the formula region on its page. `None` for markup sources.
    ///
    /// For PDF sources the coordinates are in PDF points. For image sources the
    /// coordinates are in source-image pixels. The C FFI reports an absent bbox
    /// as a null pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<BoundingBox>,

    /// 1-indexed page number the formula appears on. `None` when the source
    /// format has no page concept. The C FFI reports an absent page as `0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}
