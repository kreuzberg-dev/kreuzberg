//! Markdown footnote and citation parsing API.
//!
//! Provides utilities for parsing standard markdown footnotes and structured citation conventions.
//!
//! # Features
//!
//! - `markdown-footnotes`: Enable markdown footnote parsing
//!
//! # Examples
//!
//! ```ignore
//! # use xberg::text::markdown_footnotes::{find_footnote_anchors, parse_footnote_definitions};
//! let markdown = r#"
//! This is text with a footnote.[^1]
//!
//! [^1]: This is the footnote content.
//! "#;
//!
//! let anchors = find_footnote_anchors(markdown);
//! let definitions = parse_footnote_definitions(markdown);
//! ```

pub mod config;
pub mod parser;
pub mod types;

pub use config::FootnoteConfig;
pub use types::{Citation, FootnoteAnchor, FootnoteDefinition};

/// Find all footnote anchor references in markdown text.
///
/// Returns a vector of footnote anchors (`[^label]` use-sites), including byte offsets.
/// Footnote definitions (`[^label]: ...`) are NOT included in the results.
///
/// # Arguments
///
/// * `markdown` - The markdown text to search
///
/// # Returns
///
/// A vector of `FootnoteAnchor` entries, each with the label and byte offset.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::find_footnote_anchors;
/// let text = "Text[^src1] more text[^src2].";
/// let anchors = find_footnote_anchors(text);
/// assert_eq!(anchors.len(), 2);
/// assert_eq!(anchors[0].label, "src1");
/// assert_eq!(anchors[1].label, "src2");
/// ```
pub fn find_footnote_anchors(markdown: &str) -> Vec<FootnoteAnchor> {
    parser::find_footnote_anchors(markdown)
}

/// Parse footnote definitions from markdown text.
///
/// Returns a vector of footnote definitions found in the markdown.
/// Handles multi-line definitions with continuation/indented lines (CommonMark format).
///
/// # Arguments
///
/// * `markdown` - The markdown text to search
///
/// # Returns
///
/// A vector of `FootnoteDefinition` entries, each with label, content, and byte offset.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::parse_footnote_definitions;
/// let text = r#"[^1]: First footnote.
/// [^2]: Second footnote.
///   Continued line."#;
/// let defs = parse_footnote_definitions(text);
/// assert_eq!(defs.len(), 2);
/// ```
pub fn parse_footnote_definitions(markdown: &str) -> Vec<FootnoteDefinition> {
    parser::parse_footnote_definitions(markdown)
}

/// Find inference markers in markdown text.
///
/// Returns byte offsets of every `[*inference*]` marker found in the text.
///
/// # Arguments
///
/// * `markdown` - The markdown text to search
///
/// # Returns
///
/// A vector of byte offsets where inference markers appear.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::find_inference_markers;
/// let text = "A claim [*inference*] with inference marker.";
/// let offsets = find_inference_markers(text);
/// assert_eq!(offsets.len(), 1);
/// ```
pub fn find_inference_markers(markdown: &str) -> Vec<usize> {
    parser::find_inference_markers(markdown)
}

/// Find unmarked claims in markdown text.
///
/// Returns lines that assert a claim but carry neither a footnote citation anchor (`[^...]`)
/// nor an inference marker (`[*inference*]`).
///
/// The heuristic is simple: a line that contains alphabetic words, ends with sentence punctuation,
/// and is not a heading, blank line, or markup-only line is considered a claim.
/// Exclude lines that appear in the citation block (after `---` + `<!-- citations ... -->`).
///
/// # Arguments
///
/// * `markdown` - The markdown text to search
///
/// # Returns
///
/// A vector of trimmed line text strings for unmarked claims.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::find_unmarked_claims;
/// let text = r#"This is a claim without citation.
/// Another claim with citation.[^1]
/// This is a claim with inference.[*inference*]
///
/// [^1]: Citation"#;
/// let unmarked = find_unmarked_claims(text);
/// assert_eq!(unmarked.len(), 1);
/// assert!(unmarked[0].contains("without citation"));
/// ```
pub fn find_unmarked_claims(markdown: &str) -> Vec<String> {
    parser::find_unmarked_claims(markdown)
}

/// Parse the structured citation block from markdown.
///
/// Extracts citations from the block after a `---` thematic break followed by
/// `<!-- citations ... -->` comment. Parses each entry as:
/// `[^srcN]: <source>, <optional-locator>, excerpt: "<text>"`
///
/// Returns parsed citations with source, optional locator, and optional excerpt.
///
/// # Arguments
///
/// * `markdown` - The markdown text to search
///
/// # Returns
///
/// A vector of `Citation` entries parsed from the citation block.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::parse_citations;
/// let text = r#"Body text.
///
/// ---
/// <!-- citations -->
/// [^src1]: docs/paper.pdf, page 3, excerpt: "Exact quoted text."
/// "#;
/// let citations = parse_citations(text);
/// assert_eq!(citations.len(), 1);
/// assert_eq!(citations[0].source, "docs/paper.pdf");
/// assert_eq!(citations[0].locator, Some("page 3".to_string()));
/// ```
pub fn parse_citations(markdown: &str) -> Vec<Citation> {
    parser::parse_citations(markdown)
}

/// Verify that an excerpt appears verbatim in source text.
///
/// Performs exact matching by default. Also tries whitespace-normalized matching
/// (collapsing runs of whitespace on both sides) since PDF-extracted text often
/// has irregular spacing.
///
/// # Arguments
///
/// * `excerpt` - The text snippet to find
/// * `source_text` - The full source text to search
///
/// # Returns
///
/// `true` if the excerpt appears (exactly or with normalized whitespace), `false` otherwise.
///
/// # Examples
///
/// ```rust
/// # use xberg::text::markdown_footnotes::verify_excerpt;
/// let source = "The document states: Exact quoted text.";
/// let excerpt = "Exact quoted text";
/// assert!(verify_excerpt(excerpt, source));
///
/// // Whitespace normalization
/// let source2 = "Text with  irregular   spacing.";
/// let excerpt2 = "Text with irregular spacing";
/// assert!(verify_excerpt(excerpt2, source2));
/// ```
pub fn verify_excerpt(excerpt: &str, source_text: &str) -> bool {
    parser::verify_excerpt(excerpt, source_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_footnote_anchors_single() {
        let text = "Text[^label]";
        let anchors = find_footnote_anchors(text);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].label, "label");
        assert_eq!(anchors[0].offset, 4);
    }

    #[test]
    fn test_find_footnote_anchors_multiple() {
        let text = "First[^ref1] and second[^ref2].";
        let anchors = find_footnote_anchors(text);
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].label, "ref1");
        assert_eq!(anchors[1].label, "ref2");
    }

    #[test]
    fn test_find_footnote_anchors_with_numbers() {
        let text = "Note[^123] and [^fn-name].";
        let anchors = find_footnote_anchors(text);
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].label, "123");
        assert_eq!(anchors[1].label, "fn-name");
    }

    #[test]
    fn test_find_footnote_anchors_excludes_definitions() {
        let text = "A claim.[^1]\n\n[^1]: The footnote definition.";
        let anchors = find_footnote_anchors(text);
        assert_eq!(anchors.len(), 1, "definition site must not be reported as an anchor");
        assert_eq!(anchors[0].label, "1");
        assert_eq!(anchors[0].offset, 8);
    }

    #[test]
    fn test_find_unmarked_claims_ignores_definition_lines() {
        let text = "[^1]: This is the footnote content.";
        let unmarked = find_unmarked_claims(text);
        assert_eq!(unmarked.len(), 0);
    }

    #[test]
    fn test_parse_footnote_definitions_single() {
        let text = "[^1]: Single line footnote.";
        let defs = parse_footnote_definitions(text);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].label, "1");
        assert_eq!(defs[0].content, "Single line footnote.");
    }

    #[test]
    fn test_parse_footnote_definitions_multiple() {
        let text = r#"[^1]: First footnote.
[^2]: Second footnote."#;
        let defs = parse_footnote_definitions(text);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].label, "1");
        assert_eq!(defs[1].label, "2");
    }

    #[test]
    fn test_parse_footnote_definitions_multiline() {
        let text = r#"[^1]: First line.
  Continuation line."#;
        let defs = parse_footnote_definitions(text);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].label, "1");
        assert!(defs[0].content.contains("First line"));
        assert!(defs[0].content.contains("Continuation"));
    }

    #[test]
    fn test_find_inference_markers_single() {
        let text = "Claim[*inference*] here.";
        let offsets = find_inference_markers(text);
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0], 5);
    }

    #[test]
    fn test_find_inference_markers_multiple() {
        let text = "First[*inference*] and second[*inference*].";
        let offsets = find_inference_markers(text);
        assert_eq!(offsets.len(), 2);
    }

    #[test]
    fn test_find_inference_markers_none() {
        let text = "No markers here.";
        let offsets = find_inference_markers(text);
        assert_eq!(offsets.len(), 0);
    }

    #[test]
    fn test_find_unmarked_claims_none() {
        let text = "This is a claim.[^1]\n\n[^1]: Citation";
        let unmarked = find_unmarked_claims(text);
        assert_eq!(unmarked.len(), 0);
    }

    #[test]
    fn test_find_unmarked_claims_with_inference() {
        let text = "Claim with inference[*inference*].";
        let unmarked = find_unmarked_claims(text);
        assert_eq!(unmarked.len(), 0);
    }

    #[test]
    fn test_find_unmarked_claims_actual_claim() {
        let text = "This is an unmarked claim.";
        let unmarked = find_unmarked_claims(text);
        assert_eq!(unmarked.len(), 1);
        assert_eq!(unmarked[0].trim(), "This is an unmarked claim.");
    }

    #[test]
    fn test_find_unmarked_claims_ignores_blank_lines() {
        let text = "This is a claim.\n\n\nAnother claim.";
        let unmarked = find_unmarked_claims(text);
        assert_eq!(unmarked.len(), 2);
    }

    #[test]
    fn test_parse_citations_empty() {
        let text = "Body only.";
        let citations = parse_citations(text);
        assert_eq!(citations.len(), 0);
    }

    #[test]
    fn test_parse_citations_full() {
        let text = r#"Body text.

---
<!-- citations -->
[^src1]: docs/paper.pdf, page 3, excerpt: "Exact text here"
"#;
        let citations = parse_citations(text);
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].label, "src1");
        assert_eq!(citations[0].source, "docs/paper.pdf");
        assert_eq!(citations[0].locator, Some("page 3".to_string()));
        assert_eq!(citations[0].excerpt, Some("Exact text here".to_string()));
    }

    #[test]
    fn test_parse_citations_source_only() {
        let text = r#"---
<!-- citations -->
[^ref1]: /path/to/file.txt
"#;
        let citations = parse_citations(text);
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source, "/path/to/file.txt");
        assert_eq!(citations[0].locator, None);
        assert_eq!(citations[0].excerpt, None);
    }

    #[test]
    fn test_parse_citations_source_and_locator() {
        let text = r#"---
<!-- citations -->
[^ref1]: paper.pdf, section 2.1
"#;
        let citations = parse_citations(text);
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source, "paper.pdf");
        assert_eq!(citations[0].locator, Some("section 2.1".to_string()));
        assert_eq!(citations[0].excerpt, None);
    }

    #[test]
    fn test_verify_excerpt_exact_match() {
        let source = "The document contains exact quoted text here.";
        let excerpt = "exact quoted text";
        assert!(verify_excerpt(excerpt, source));
    }

    #[test]
    fn test_verify_excerpt_not_found() {
        let source = "The document contains some text.";
        let excerpt = "missing text";
        assert!(!verify_excerpt(excerpt, source));
    }

    #[test]
    fn test_verify_excerpt_whitespace_normalized() {
        let source = "Text  with   irregular    spacing.";
        let excerpt = "Text with irregular spacing";
        assert!(verify_excerpt(excerpt, source));
    }

    #[test]
    fn test_verify_excerpt_case_sensitive() {
        let source = "Text With Capital Letters.";
        let excerpt = "text with capital letters";
        assert!(!verify_excerpt(excerpt, source));
    }

    #[test]
    fn test_verify_excerpt_empty_excerpt() {
        let source = "Some text.";
        let excerpt = "";
        assert!(verify_excerpt(excerpt, source));
    }

    #[test]
    fn test_serde_roundtrip_footnote_anchor() {
        let anchor = FootnoteAnchor {
            label: "test".to_string(),
            offset: 42,
        };
        let json = serde_json::to_string(&anchor).unwrap();
        let deserialized: FootnoteAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.label, anchor.label);
        assert_eq!(deserialized.offset, anchor.offset);
    }

    #[test]
    fn test_serde_roundtrip_footnote_definition() {
        let def = FootnoteDefinition {
            label: "1".to_string(),
            content: "Definition content".to_string(),
            offset: 10,
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: FootnoteDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.label, def.label);
        assert_eq!(deserialized.content, def.content);
    }

    #[test]
    fn test_serde_roundtrip_citation() {
        let citation = Citation {
            label: "src1".to_string(),
            source: "paper.pdf".to_string(),
            locator: Some("page 1".to_string()),
            excerpt: Some("quoted text".to_string()),
        };
        let json = serde_json::to_string(&citation).unwrap();
        let deserialized: Citation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.label, citation.label);
        assert_eq!(deserialized.source, citation.source);
        assert_eq!(deserialized.locator, citation.locator);
        assert_eq!(deserialized.excerpt, citation.excerpt);
    }
}
