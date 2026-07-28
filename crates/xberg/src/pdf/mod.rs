//! Pure-Rust PDF document processing via the `pdf-oxide` backend.
//!
//! Used internally by the PDF extractor plugin. Requires the `pdf` feature.
//!
//! # Features
//!
//! - **Text extraction**: Extract text content from PDFs using `pdf_oxide`
//! - **Metadata extraction**: Parse PDF metadata (title, author, creation date, etc.)
//! - **Image extraction**: Extract embedded images from PDF pages
//! - **Error handling**: Comprehensive PDF-specific error types
#[cfg(feature = "pdf")]
/// PDF bookmark (outline/table-of-contents) extraction.
pub mod bookmarks;
#[cfg(feature = "pdf")]
/// Embedded file extraction from PDF portfolios and attachments.
pub mod embedded_files;
#[cfg(feature = "pdf")]
/// PDF-specific error types.
pub mod error;
#[cfg(feature = "pdf")]
/// Document hierarchy reconstruction from PDF structure trees.
pub mod hierarchy;
#[cfg(all(feature = "pdf", feature = "layout-detection"))]
/// Per-page pre-screen for adaptive layout detection.
pub(crate) mod layout_gate;
#[cfg(feature = "pdf")]
/// PDF metadata types: document info dictionary and page structure.
pub mod metadata;
#[cfg(feature = "pdf")]
pub(crate) mod oxide;
#[cfg(all(feature = "pdf", feature = "tokio-runtime"))]
pub(crate) mod oxide_text;
#[cfg(feature = "pdf")]
/// PDF page rendering to raster images.
pub mod render;
#[cfg(feature = "pdf")]
/// Scanned-page detection.
pub(crate) mod scan_detect;
#[cfg(feature = "pdf")]
/// PDF logical structure extraction (tagged PDF support).
pub mod structure;
#[cfg(feature = "pdf")]
/// Table reconstruction from PDF text-layer word positions.
pub mod table_reconstruct;
#[cfg(feature = "pdf")]
pub(crate) mod text;
#[cfg(feature = "pdf")]
pub(crate) mod xref_revisions;

#[cfg(feature = "pdf")]
pub use crate::core::config::HierarchyConfig;
#[cfg(feature = "pdf")]
pub use error::PdfError;
