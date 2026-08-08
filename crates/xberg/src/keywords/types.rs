//! Shared types for keyword extraction.

use serde::{Deserialize, Serialize};

/// Keyword algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum KeywordAlgorithm {
    /// YAKE (Yet Another Keyword Extractor) - statistical approach
    #[cfg(feature = "keywords-yake")]
    Yake,

    /// RAKE (Rapid Automatic Keyword Extraction) - co-occurrence based
    #[cfg(feature = "keywords-rake")]
    Rake,
}

impl Default for KeywordAlgorithm {
    fn default() -> Self {
        #[cfg(feature = "keywords-yake")]
        return Self::Yake;

        #[cfg(all(feature = "keywords-rake", not(feature = "keywords-yake")))]
        return Self::Rake;

        #[cfg(not(any(feature = "keywords-yake", feature = "keywords-rake")))]
        compile_error!("At least one keyword extraction feature must be enabled");
    }
}

/// Extracted keyword with metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct Keyword {
    /// The keyword text.
    pub text: String,

    /// Relevance score (higher is better, algorithm-specific range).
    pub score: f32,

    /// Algorithm that extracted this keyword.
    pub algorithm: KeywordAlgorithm,

    /// Optional positions where keyword appears in text (character offsets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positions: Option<Vec<usize>>,
}

impl Keyword {
    /// Create a new keyword, populating `positions` with every character
    /// offset at which `text` occurs (case-insensitively) in `source` (#266).
    ///
    /// `positions` is `None` when `text` cannot be located in `source` at
    /// all — e.g. an algorithm normalised or stemmed the keyword text before
    /// returning it — rather than a misleading empty list.
    pub(crate) fn with_positions(text: String, score: f32, algorithm: KeywordAlgorithm, source: &str) -> Self {
        let positions = find_positions(source, &text);
        Self {
            text,
            score,
            algorithm,
            positions,
        }
    }
}

/// Find every character offset at which `needle` occurs in `haystack`,
/// comparing ASCII case-insensitively (matches typical keyword casing
/// normalisation without corrupting offsets via full Unicode case-folding,
/// which can change a match's character count for some scripts).
///
/// Returns `None` when there is no match, and `Some` with one entry per
/// (possibly overlapping) occurrence otherwise, in ascending order.
pub(crate) fn find_positions(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.is_empty() {
        return None;
    }

    let haystack_chars: Vec<char> = haystack.chars().collect();
    if haystack_chars.len() < needle_chars.len() {
        return None;
    }

    let mut positions = Vec::new();
    for start in 0..=(haystack_chars.len() - needle_chars.len()) {
        let is_match = haystack_chars[start..start + needle_chars.len()]
            .iter()
            .zip(&needle_chars)
            .all(|(h, n)| h.eq_ignore_ascii_case(n));
        if is_match {
            positions.push(start);
        }
    }

    if positions.is_empty() { None } else { Some(positions) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_find_positions_returns_none_for_empty_needle() {
        assert_eq!(find_positions("hello world", ""), None);
    }

    #[test]
    fn should_find_positions_returns_none_when_absent() {
        assert_eq!(find_positions("hello world", "xyz"), None);
    }

    #[test]
    fn should_find_positions_locates_single_occurrence() {
        assert_eq!(find_positions("machine learning is great", "learning"), Some(vec![8]));
    }

    #[test]
    fn should_find_positions_is_case_insensitive() {
        assert_eq!(find_positions("Machine Learning", "learning"), Some(vec![8]));
    }

    #[test]
    fn should_find_positions_locates_every_occurrence() {
        assert_eq!(
            find_positions("rust is rust, and rust rocks", "rust"),
            Some(vec![0, 8, 18])
        );
    }

    #[test]
    fn should_with_positions_populates_offsets() {
        let keyword = Keyword::with_positions(
            "learning".to_string(),
            0.5,
            // Both variants are feature-gated, so naming one breaks any build without it;
            // these tests are about position offsets, not which algorithm produced them.
            KeywordAlgorithm::default(),
            "machine learning is great",
        );
        assert_eq!(keyword.positions, Some(vec![8]));
    }

    #[test]
    fn should_with_positions_is_none_when_keyword_not_found_verbatim() {
        let keyword = Keyword::with_positions(
            "learn*".to_string(),
            0.5,
            // Both variants are feature-gated, so naming one breaks any build without it;
            // these tests are about position offsets, not which algorithm produced them.
            KeywordAlgorithm::default(),
            "machine learning is great",
        );
        assert_eq!(keyword.positions, None);
    }
}
