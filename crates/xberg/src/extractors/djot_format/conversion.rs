//! Djot content conversion and HTML rendering APIs.
//!
//! Provides public APIs for converting between different representations:
//! - DjotContent to djot markup
//! - ExtractedDocument to djot markup
//! - Djot markup to HTML

use super::rendering::render_block_to_djot;
#[cfg(test)]
use jotdown::Parser;
#[cfg(test)]
use std::borrow::Cow;

/// Convert DjotContent back to djot markup.
///
/// This function takes a `DjotContent` structure and generates valid djot markup
/// from it, preserving:
/// - Block structure (headings, code blocks, lists, blockquotes, etc.)
/// - Inline formatting (strong, emphasis, highlight, subscript, superscript, etc.)
/// - Attributes where present ({.class #id key="value"})
///
/// # Arguments
///
/// * `content` - The DjotContent to convert
///
/// # Returns
///
/// A String containing valid djot markup
///
/// # Example
///
/// ```ignore
/// let djot_content = // ... extract from some source
/// let markup = djot_content_to_djot(&djot_content);
/// println!("{}", markup);
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn djot_content_to_djot(content: &crate::types::DjotContent) -> String {
    let mut output = String::new();

    for block in &content.blocks {
        output.push_str(&render_block_to_djot(block, 0));
    }

    output
}

/// Convert any ExtractedDocument to djot format.
///
/// This function converts an `ExtractedDocument` to djot markup:
/// - If `djot_content` is `Some`, uses `djot_content_to_djot` for full fidelity conversion
/// - Otherwise, wraps the plain text content in paragraphs
///
/// # Arguments
///
/// * `result` - The ExtractedDocument to convert
///
/// # Returns
///
/// A `Result` containing the djot markup string
///
/// # Example
///
/// ```ignore
/// let result = extractor.extract_bytes(bytes, "text/plain", &config).await?;
/// let djot_markup = extraction_result_to_djot(&result)?;
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn extraction_result_to_djot(result: &crate::types::ExtractedDocument) -> crate::Result<String> {
    if let Some(ref djot_content) = result.djot_content {
        Ok(djot_content_to_djot(djot_content))
    } else {
        let mut output = String::new();

        // `ExtractedDocument::content` carries no LF-only guarantee: `render_plain`
        // copies each element's text verbatim (crate::rendering::plain) and the
        // `pre_rendered_content` path bypasses rendering entirely, so a CRLF-authored
        // source (e.g. a Windows-authored code file) reaches here with `\r\n` intact.
        // Splitting raw emits the whole document as one djot paragraph (#316).
        let content = crate::extraction::transform::normalize_line_endings(&result.content);
        let paragraphs: Vec<&str> = content.split("\n\n").collect();

        for para in paragraphs {
            let trimmed = para.trim();
            if !trimmed.is_empty() {
                output.push_str(trimmed);
                output.push_str("\n\n");
            }
        }

        Ok(output)
    }
}

/// Render djot content to HTML.
///
/// This function takes djot source text and renders it to HTML using jotdown's
/// built-in HTML renderer.
///
/// # Arguments
///
/// * `djot_source` - The djot markup text to render
///
/// # Returns
///
/// A `Result` containing the rendered HTML string
///
/// # Example
///
/// ```ignore
/// let djot = "# Hello\n\nThis is *bold* and _italic_.";
/// let html = djot_to_html(djot)?;
/// assert!(html.contains("<h1>"));
/// assert!(html.contains("<strong>"));
/// assert!(html.contains("<em>"));
/// ```
#[cfg(test)]
pub(crate) fn djot_to_html(djot_source: &str) -> crate::Result<String> {
    let parser = Parser::new(djot_source);
    let html = jotdown::html::render_to_string(parser);
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BlockType, DjotContent, ExtractedDocument, FormattedBlock, InlineElement, InlineType, Metadata,
    };

    #[test]
    fn test_djot_content_to_djot_heading() {
        let content = DjotContent {
            plain_text: "Test Heading".to_string(),
            blocks: vec![FormattedBlock {
                block_type: BlockType::Heading,
                level: Some(1),
                inline_content: vec![InlineElement {
                    element_type: InlineType::Text,
                    content: "Test Heading".to_string(),
                    attributes: None,
                    metadata: None,
                }],
                attributes: None,
                language: None,
                code: None,
                children: vec![],
            }],
            metadata: Metadata::default(),
            tables: vec![],
            images: vec![],
            links: vec![],
            footnotes: vec![],
            attributes: Default::default(),
        };

        let markup = djot_content_to_djot(&content);
        assert!(markup.contains("# Test Heading"));
    }

    #[test]
    fn test_extraction_result_to_djot_with_djot_content() {
        let result = ExtractedDocument {
            content: "Test content".to_string(),
            mime_type: Cow::Borrowed("text/djot"),
            djot_content: Some(DjotContent {
                plain_text: "Test content".to_string(),
                blocks: vec![FormattedBlock {
                    block_type: BlockType::Paragraph,
                    level: None,
                    inline_content: vec![InlineElement {
                        element_type: InlineType::Text,
                        content: "Test content".to_string(),
                        attributes: None,
                        metadata: None,
                    }],
                    attributes: None,
                    language: None,
                    code: None,
                    children: vec![],
                }],
                metadata: Metadata::default(),
                tables: vec![],
                images: vec![],
                links: vec![],
                footnotes: vec![],
                attributes: Default::default(),
            }),
            ..Default::default()
        };

        let markup = extraction_result_to_djot(&result).expect("Should convert");
        assert!(markup.contains("Test content"));
    }

    #[test]
    fn test_extraction_result_to_djot_without_djot_content() {
        let result = ExtractedDocument {
            content: "Paragraph one\n\nParagraph two".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        let markup = extraction_result_to_djot(&result).expect("Should convert");
        assert!(markup.contains("Paragraph one"));
        assert!(markup.contains("Paragraph two"));
    }

    #[test]
    fn test_djot_to_html_heading() {
        let djot = "# Hello";
        let html = djot_to_html(djot).expect("Should render");
        assert!(html.contains("<h1>") || html.contains("<H1>"));
    }

    #[test]
    fn test_djot_to_html_formatting() {
        let djot = "This is *bold* and _italic_.";
        let html = djot_to_html(djot).expect("Should render");
        assert!(html.contains("<strong>") || html.contains("<em>"));
    }
}
