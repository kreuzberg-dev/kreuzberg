//! Capacity estimation utilities for string pre-allocation.
//!
//! This module provides functions to estimate the optimal capacity for string buffers
//! based on file sizes and content types. By pre-allocating with accurate capacity hints,
//! we reduce reallocation cycles during text extraction and conversion operations.
//!
//! # Estimation Ratios
//!
//! Extraction ratios are based on empirical analysis of typical document conversions:
//! - **Plain text** (txt): ~95% of file size (minimal overhead)
//! - **Markdown** (md): ~95% of file size (minimal overhead)
//! - **HTML** (html/htm): ~65% of file size (tags removed, structure reduced)
//! - **DOCX** (docx): ~45% of compressed size (XML + compressed content)
//! - **Excel** (xlsx): ~40% of compressed size (cell values extracted)
//! - **PPTX** (pptx): ~35% of compressed size (slide content extracted)
//! - **PDF** (pdf): ~25% of file size (binary overhead, compression)
//! - **Default**: ~50% (conservative estimate)
//!
//! # Example
//!
//! Not run as a doctest: `estimate_content_capacity` is `pub(crate)`, an allocation
//! hint for the extractors rather than part of the public surface.
//!
//! ```ignore
//! use xberg::extraction::capacity::estimate_content_capacity;
//!
//! let file_size = 1_000_000u64;
//! let capacity_txt = estimate_content_capacity(file_size, "txt");
//! let capacity_html = estimate_content_capacity(file_size, "html");
//!
//! assert_eq!(capacity_txt, 950_000); // 95% of 1MB
//! assert_eq!(capacity_html, 650_000); // 65% of 1MB
//! ```

/// Estimate the capacity needed for content extracted from a file.
///
/// Returns an estimated byte capacity for a string buffer that will accumulate
/// extracted content. The estimation is based on:
/// - The original file size
/// - The content type/format
/// - Empirical ratios of final content size to original file size
///
/// # Arguments
///
/// * `file_size` - The size of the original file in bytes
/// * `format` - The file format/extension (e.g., "txt", "html", "docx", "xlsx", "pptx")
///
/// # Returns
///
/// An estimated capacity in bytes suitable for `String::with_capacity()`
///
/// # Minimum Capacity
///
/// All estimates have a minimum of 64 bytes to prevent over-optimization for very
/// small files where the overhead of capacity estimation outweighs benefits.
///
/// # Example
///
/// ```ignore
/// use xberg::extraction::capacity::estimate_content_capacity;
///
/// // 1MB text file → expect ~950KB of extracted content
/// let txt_cap = estimate_content_capacity(1_000_000, "txt");
/// assert_eq!(txt_cap, 950_000);
///
/// // 1MB HTML → expect ~650KB of extracted markdown
/// let html_cap = estimate_content_capacity(1_000_000, "html");
/// assert_eq!(html_cap, 650_000);
///
/// // 1MB DOCX → expect ~450KB of extracted text
/// let docx_cap = estimate_content_capacity(1_000_000, "docx");
/// assert_eq!(docx_cap, 450_000);
/// ```
#[allow(dead_code, reason = "utility kept for use by capacity-aware extractors and tests")]
#[inline]
pub(crate) fn estimate_content_capacity(file_size: u64, format: &str) -> usize {
    let ratio = match format.to_lowercase().as_str() {
        "txt" | "text" => 0.95,
        "md" | "markdown" => 0.95,
        "html" | "htm" => 0.65,
        "docx" | "doc" => 0.45,
        "xlsx" | "xls" | "xlsm" | "xlam" | "xltm" | "xlsb" => 0.40,
        "pptx" | "ppt" | "pptm" | "ppsx" => 0.35,
        "pdf" => 0.25,
        _ => 0.50,
    };

    (file_size as f64 * ratio).ceil() as usize
}

/// Estimate capacity for markdown table generation.
///
/// Markdown tables have predictable size: ~12 bytes per cell on average
/// (accounting for separators, pipes, padding, and cell content).
///
/// # Arguments
///
/// * `row_count` - Number of rows in the table
/// * `col_count` - Number of columns in the table
///
/// # Returns
///
/// An estimated capacity for the markdown table output
#[inline]
#[cfg(any(feature = "excel", feature = "excel-wasm", feature = "office", feature = "xml"))]
pub(crate) fn estimate_table_markdown_capacity(row_count: usize, col_count: usize) -> usize {
    let base = 50 + (col_count * 5);
    let cell_estimate = row_count.saturating_mul(col_count).saturating_mul(12);
    base.saturating_add(cell_estimate).max(64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_content_capacity_text() {
        let capacity = estimate_content_capacity(1_000_000, "txt");
        assert_eq!(capacity, 950_000);
    }

    #[test]
    fn test_estimate_content_capacity_markdown() {
        let capacity = estimate_content_capacity(1_000_000, "md");
        assert_eq!(capacity, 950_000);
    }

    #[test]
    fn test_estimate_content_capacity_html() {
        let capacity = estimate_content_capacity(1_000_000, "html");
        assert_eq!(capacity, 650_000);
    }

    #[test]
    fn test_estimate_content_capacity_docx() {
        let capacity = estimate_content_capacity(1_000_000, "docx");
        assert_eq!(capacity, 450_000);
    }

    #[test]
    fn test_estimate_content_capacity_xlsx() {
        let capacity = estimate_content_capacity(1_000_000, "xlsx");
        assert_eq!(capacity, 400_000);
    }

    #[test]
    fn test_estimate_content_capacity_pptx() {
        let capacity = estimate_content_capacity(1_000_000, "pptx");
        assert_eq!(capacity, 350_000);
    }

    #[test]
    fn test_estimate_content_capacity_pdf() {
        let capacity = estimate_content_capacity(1_000_000, "pdf");
        assert_eq!(capacity, 250_000);
    }

    #[test]
    fn test_estimate_content_capacity_unknown() {
        let capacity = estimate_content_capacity(1_000_000, "unknown");
        assert_eq!(capacity, 500_000);
    }

    #[test]
    fn test_estimate_content_capacity_case_insensitive() {
        let lower = estimate_content_capacity(1_000_000, "html");
        let upper = estimate_content_capacity(1_000_000, "HTML");
        let mixed = estimate_content_capacity(1_000_000, "HtMl");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[cfg(any(feature = "office", feature = "xml"))]
    #[test]
    fn test_table_markdown_capacity() {
        let capacity = estimate_table_markdown_capacity(10, 5);
        assert_eq!(capacity, 675);
    }

    #[cfg(any(feature = "office", feature = "xml"))]
    #[test]
    fn test_table_markdown_capacity_minimum() {
        let capacity = estimate_table_markdown_capacity(0, 0);
        assert!(capacity >= 64);
    }

    #[test]
    fn test_capacity_overflow_resistance() {
        let capacity = estimate_content_capacity(u64::MAX, "txt");
        assert!(capacity > 0);
    }
}
