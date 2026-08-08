#[cfg(any(feature = "pdf", feature = "office", feature = "ocr"))]
pub mod blank_detection;
pub mod derive;
pub(crate) mod doctags;
#[cfg(any(feature = "html", feature = "email"))]
pub(crate) mod grid_flatten;
pub mod image_kind;
pub mod structured;
pub mod text;
pub mod transform;

#[cfg(feature = "hwp")]
pub mod hwp;

#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
pub mod image;

/// HEIF-family (HEIC, HEIF, AVIF) detection and decoding.
///
/// The detector (`is_heif_container`) is always compiled; the decoder
/// (`decode_heic_to_png`) is gated by the `heic` feature.
pub(crate) mod heif;

/// EXIF metadata extraction via `nom-exif` (pure Rust).
///
/// Available under any of `ocr`, `ocr-wasm`, or `heic` so the same tag set
/// reaches every target without re-implementing the bridge per surface.
pub(crate) mod exif;

/// Capacity estimation utilities for string pre-allocation.
///
/// This module provides functions to estimate the capacity needed for string buffers
/// based on input file sizes and content types. This enables pre-allocation, reducing
/// reallocation cycles during string building operations.
pub mod capacity;

#[cfg(feature = "archives")]
pub mod archive;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "email")]
pub mod pst;

#[cfg(any(feature = "excel", feature = "excel-wasm"))]
pub mod excel;

#[cfg(feature = "html")]
pub mod html;

#[cfg(feature = "office")]
pub mod doc;

#[cfg(feature = "office")]
pub mod docx;

#[cfg(feature = "office")]
pub mod mathml;

/// Unicode-to-LaTeX symbol table shared by the OMML (`docx::math`) and MathML
/// (`mathml`) converters.
#[cfg(feature = "office")]
pub(crate) mod math_symbols;

#[cfg(feature = "office")]
pub mod office_metadata;

#[cfg(feature = "office")]
pub mod ooxml_constants;

#[cfg(feature = "office")]
pub mod ooxml_embedded;

#[cfg(feature = "office")]
pub mod image_format;

#[cfg(all(feature = "ocr", feature = "tokio-runtime"))]
pub mod image_ocr;

#[cfg(feature = "office")]
pub mod ppt;

#[cfg(feature = "office")]
pub mod pptx;

#[cfg(feature = "xml")]
pub mod xml;

#[cfg(any(feature = "office", feature = "xml"))]
pub mod markdown;

#[cfg(feature = "html")]
pub use html::convert_html_to_markdown;

#[cfg(any(feature = "office", feature = "xml"))]
pub(crate) use markdown::cells_to_markdown;
#[cfg(any(feature = "office", feature = "xml"))]
pub(crate) use markdown::cells_to_text;
