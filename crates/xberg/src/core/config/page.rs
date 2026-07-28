//! Page extraction and tracking configuration.
//!
//! Controls how pages are extracted, tracked, and represented in extraction results.
//! When `None`, page tracking is disabled.

use serde::{Deserialize, Serialize};

/// Page extraction and tracking configuration.
///
/// Controls how pages are extracted, tracked, and represented in the extraction results.
/// When `None`, page tracking is disabled.
///
/// Page range tracking in chunk metadata (first_page/last_page) is automatically enabled
/// when page boundaries are available and chunking is configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PageConfig {
    /// Extract pages as separate array (ExtractedDocument.pages)
    #[serde(default)]
    pub extract_pages: bool,

    /// Insert page markers in main content string
    #[serde(default)]
    pub insert_page_markers: bool,

    /// Page marker format (use {page_num} placeholder)
    /// Default: "\n\n<!-- PAGE {page_num} -->\n\n"
    #[serde(default = "default_page_marker_format")]
    pub marker_format: String,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            extract_pages: false,
            insert_page_markers: false,
            marker_format: "\n\n<!-- PAGE {page_num} -->\n\n".to_string(),
        }
    }
}

fn default_page_marker_format() -> String {
    "\n\n<!-- PAGE {page_num} -->\n\n".to_string()
}

/// Regex matching one whole line that is a rendered instance of `marker_format`
/// (every `{page_num}` placeholder replaced by digits). Used to recognize page
/// marker lines so renderers pass them through verbatim.
pub(crate) fn marker_line_regex(marker_format: &str) -> regex::Regex {
    let pattern = marker_format
        .trim()
        .split("{page_num}")
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"\d+");
    regex::Regex::new(&format!("^{pattern}$")).expect("escaped marker format is a valid regex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_config_default() {
        let config = PageConfig::default();
        assert!(!config.extract_pages);
        assert!(!config.insert_page_markers);
        assert_eq!(config.marker_format, "\n\n<!-- PAGE {page_num} -->\n\n");
    }
}
