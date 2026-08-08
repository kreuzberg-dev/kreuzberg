//! CSV/TSV extraction configuration.

use serde::{Deserialize, Serialize};

/// Configuration for CSV/TSV extraction.
///
/// When unset (`ExtractionConfig::csv == None`), the extractor keeps its
/// existing default behavior: the delimiter is auto-detected by sampling the
/// file (comma, tab, pipe, or semicolon), and no line is treated as a comment.
///
/// # Example
///
/// ```rust
/// use xberg::core::config::CsvConfig;
///
/// let config = CsvConfig {
///     delimiter: Some(";".to_string()),
///     comment_prefixes: vec!["#".to_string()],
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct CsvConfig {
    /// Field delimiter, as a single-character string (e.g. `","`, `";"`,
    /// `"\t"`, `"|"`). When `None` (default), the delimiter is auto-detected
    /// from a sample of the file.
    ///
    /// Must be exactly one ASCII byte when set — `ExtractionConfig::validate`
    /// rejects an empty string or a multi-byte value with a helpful error.
    /// The TSV MIME type (`text/tab-separated-values`) always forces `\t`
    /// regardless of this setting.
    pub delimiter: Option<String>,

    /// Line prefixes that mark a comment line to skip entirely during row
    /// parsing (e.g. `["#"]`). A line is treated as a comment when its
    /// trimmed start matches any of these prefixes exactly.
    ///
    /// Default: empty, meaning no line is treated as a comment (matches the
    /// pre-existing extractor behavior).
    pub comment_prefixes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_delimiter_and_no_comment_prefixes() {
        let config = CsvConfig::default();
        assert!(config.delimiter.is_none());
        assert!(config.comment_prefixes.is_empty());
    }

    #[test]
    fn serde_roundtrip() {
        let config = CsvConfig {
            delimiter: Some(";".to_string()),
            comment_prefixes: vec!["#".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: CsvConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.delimiter.as_deref(), Some(";"));
        assert_eq!(back.comment_prefixes, vec!["#".to_string()]);
    }

    #[test]
    fn serde_default_omitted() {
        let config: CsvConfig = serde_json::from_str("{}").unwrap();
        assert!(config.delimiter.is_none());
        assert!(config.comment_prefixes.is_empty());
    }
}
