//! Cross-extractor content filtering configuration.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Cross-extractor content filtering configuration.
///
/// Controls whether "furniture" content (headers, footers, page numbers,
/// watermarks, repeating text) is included in or stripped from extraction
/// results. Applies across all extractors (PDF, DOCX, RTF, ODT, HTML, etc.)
/// with format-specific implementation.
///
/// When `None` on `ExtractionConfig`, each extractor uses its current
/// default behavior unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentFilterConfig {
    /// Include running headers in extraction output.
    ///
    /// - PDF: Disables top-margin furniture stripping and prevents the layout
    ///   model from treating `PageHeader`-classified regions as furniture.
    /// - DOCX: Includes document headers in text output.
    /// - RTF/ODT: Headers already included; this is a no-op when true.
    /// - HTML/EPUB: Keeps `<header>` element content.
    ///
    /// Default: `false` (headers are stripped or excluded).
    #[serde(default)]
    pub include_headers: bool,

    /// Include running footers in extraction output.
    ///
    /// - PDF: Disables bottom-margin furniture stripping and prevents the layout
    ///   model from treating `PageFooter`-classified regions as furniture.
    /// - DOCX: Includes document footers in text output.
    /// - RTF/ODT: Footers already included; this is a no-op when true.
    /// - HTML/EPUB: Keeps `<footer>` element content.
    ///
    /// Default: `false` (footers are stripped or excluded).
    #[serde(default)]
    pub include_footers: bool,

    /// Include footnote bodies in extraction output.
    ///
    /// - PDF: Prevents the layout model from treating `Footnote`-classified
    ///   regions as furniture, so footnote bodies survive alongside the main
    ///   text instead of being silently dropped.
    /// - Other formats: No effect currently.
    ///
    /// Default: `false` (footnotes are stripped), matching the existing
    /// `include_headers` / `include_footers` defaults.
    #[serde(default)]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub include_footnotes: bool,

    /// Enable the heuristic cross-page repeating text detector.
    ///
    /// When `true` (default), text that repeats verbatim across a supermajority
    /// of pages is classified as furniture and stripped.  Disable this if brand
    /// names or repeated headings are being incorrectly removed by the heuristic.
    ///
    /// Note: when a layout-detection model is active, the model may independently
    /// classify page-header / page-footer / footnote regions as furniture on a
    /// per-page basis. To preserve those regions, set `include_headers = true`,
    /// `include_footers = true`, `include_footnotes = true`, or any combination,
    /// in addition to disabling this flag.
    ///
    /// Primarily affects PDF extraction.
    ///
    /// Default: `true`.
    #[serde(default = "default_true")]
    pub strip_repeating_text: bool,

    /// Include watermark text in extraction output.
    ///
    /// - PDF: Keeps watermark artifacts and arXiv identifiers.
    /// - Other formats: No effect currently.
    ///
    /// Default: `false` (watermarks are stripped).
    #[serde(default)]
    pub include_watermarks: bool,
}

impl Default for ContentFilterConfig {
    fn default() -> Self {
        Self {
            include_headers: false,
            include_footers: false,
            include_footnotes: false,
            strip_repeating_text: true,
            include_watermarks: false,
        }
    }
}
