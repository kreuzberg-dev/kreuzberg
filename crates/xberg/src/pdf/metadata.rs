//! PDF metadata types (backend-agnostic).
//!
//! These types are shared between the oxide extraction backend and any callers
//! that consume PDF metadata. No PDF-library-specific dependencies.

use serde::{Deserialize, Serialize};

use crate::types::PageStructure;

/// PDF-specific metadata.
///
/// Contains metadata fields specific to PDF documents that are not in the common
/// `Metadata` structure. Common fields like title, authors, keywords, and dates
/// are at the `Metadata` level.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct PdfMetadata {
    /// PDF version (e.g., "1.7", "2.0")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_version: Option<String>,

    /// PDF producer (application that created the PDF)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,

    /// Whether the PDF is encrypted/password-protected
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_encrypted: Option<bool>,

    /// First page width in points (1/72 inch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,

    /// First page height in points (1/72 inch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,

    /// Total number of pages in the PDF document
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,

    /// How strongly the document's most scan-like page resembles a scan, in `[0.0, 1.0]`.
    ///
    /// `None` when the document could not be inspected. A full-page raster with no
    /// visible text scores at least `0.85`; a born-digital slide with a full-bleed
    /// background image scores `0.50`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_confidence: Option<f32>,

    /// Pages that look like scans (1-indexed), using the default confidence threshold.
    ///
    /// `None` when the document could not be inspected; empty when no page qualifies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_pages: Option<Vec<u32>>,

    /// Pages the `auto` layout strategy skipped (1-indexed).
    ///
    /// `None` unless layout detection ran with `LayoutStrategy::Auto`; empty
    /// when the gate selected every page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_gated_pages: Option<Vec<u32>>,

    /// Why the `auto` layout gate selected or skipped each page.
    ///
    /// Index `i` is page `i + 1`. Snake_case values such as `multi_column`,
    /// `table_grid`, or `plain_text` (the skip reason). `None` unless layout
    /// detection ran with `LayoutStrategy::Auto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_gate_reasons: Option<Vec<String>>,
}
/// Complete PDF extraction metadata including common and PDF-specific fields.
///
/// Combines common document fields (title, authors, dates) with PDF-specific
/// metadata and optional page structure information.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfExtractionMetadata {
    /// Document title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Document subject or description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,

    /// Document authors (parsed from PDF Author field)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,

    /// Document keywords (parsed from PDF Keywords field)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,

    /// Creation timestamp (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,

    /// Last modification timestamp (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,

    /// Application or user that created the document
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// PDF-specific metadata
    pub pdf_specific: PdfMetadata,

    /// Page structure with boundaries and optional per-page metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_structure: Option<PageStructure>,
}
#[cfg_attr(alef, alef(skip))]
/// Common PDF metadata fields extracted from the document info dictionary.
///
/// Used as an intermediate type during extraction before building `PdfExtractionMetadata`.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct CommonPdfMetadata {
    /// Document title from the PDF Info dictionary.
    pub title: Option<String>,
    /// Document subject from the PDF Info dictionary.
    pub subject: Option<String>,
    /// Document authors parsed from the PDF Author field.
    pub authors: Option<Vec<String>>,
    /// Keywords parsed from the PDF Keywords field.
    pub keywords: Option<Vec<String>>,
    /// Creation timestamp in ISO 8601 format.
    pub created_at: Option<String>,
    /// Last modification timestamp in ISO 8601 format.
    pub modified_at: Option<String>,
    /// Creator application or author name.
    pub created_by: Option<String>,
}
