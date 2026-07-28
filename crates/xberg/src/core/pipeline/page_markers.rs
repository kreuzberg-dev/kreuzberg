//! Page marker element injection for rendered output formats.
//!
//! Extractors insert page markers into the plain-text content string, but the
//! Markdown/Djot renderers work from `InternalDocument` elements, where markers
//! either arrive as escapable `Paragraph` text (flat documents) or not at all
//! (structured documents, which carry `PageBreak` elements instead). This pass
//! normalizes both shapes to verbatim `RawBlock` marker elements.

use crate::core::config::page::marker_line_regex;
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

/// Represent page markers as `RawBlock` elements so renderers emit them verbatim.
///
/// Flat documents (built by splitting marker-bearing text into paragraphs) get
/// their marker paragraphs converted in place. Structured documents (no marker
/// text, `PageBreak` elements at page boundaries) get `RawBlock` markers
/// synthesized: one before the first element, one after each page break.
pub(crate) fn inject_page_marker_elements(doc: &mut InternalDocument, marker_format: &str) {
    if doc.elements.is_empty() {
        return;
    }

    let marker_re = marker_line_regex(marker_format);
    let mut converted = false;
    for elem in doc.elements.iter_mut() {
        if matches!(elem.kind, ElementKind::Paragraph) && marker_re.is_match(elem.text.trim()) {
            elem.kind = ElementKind::RawBlock;
            elem.text = elem.text.trim().to_string();
            converted = true;
        }
    }
    if converted {
        return;
    }

    let render_marker = |page: u32| {
        marker_format
            .replace("{page_num}", &page.to_string())
            .trim()
            .to_string()
    };
    let marker_elem = |page: u32| InternalElement::text(ElementKind::RawBlock, render_marker(page), 0);

    let elements = std::mem::take(&mut doc.elements);
    let total = elements.len();
    let mut out = Vec::with_capacity(total + 4);
    let mut current_page = elements.iter().find_map(|e| e.page).unwrap_or(1);
    out.push(marker_elem(current_page));

    let mut iter = elements.into_iter().enumerate().peekable();
    while let Some((idx, elem)) = iter.next() {
        let is_break = matches!(elem.kind, ElementKind::PageBreak);
        out.push(elem);
        if is_break && idx + 1 < total {
            current_page = iter.peek().and_then(|(_, next)| next.page).unwrap_or(current_page + 1);
            out.push(marker_elem(current_page));
        }
    }
    doc.elements = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_FORMAT: &str = "\n\n<!-- PAGE {page_num} -->\n\n";

    fn paragraph(text: &str) -> InternalElement {
        InternalElement::text(ElementKind::Paragraph, text, 0)
    }

    #[test]
    fn converts_flat_marker_paragraphs_to_raw_blocks() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("<!-- PAGE 1 -->"));
        doc.push_element(paragraph("First page text."));
        doc.push_element(paragraph("<!-- PAGE 2 -->"));
        doc.push_element(paragraph("Second page text."));

        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);

        assert!(matches!(doc.elements[0].kind, ElementKind::RawBlock));
        assert_eq!(doc.elements[0].text, "<!-- PAGE 1 -->");
        assert!(matches!(doc.elements[1].kind, ElementKind::Paragraph));
        assert!(matches!(doc.elements[2].kind, ElementKind::RawBlock));
        assert_eq!(doc.elements.len(), 4);
    }

    #[test]
    fn synthesizes_markers_around_page_breaks() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("First page text."));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(paragraph("Second page text."));

        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::RawBlock))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(texts, vec!["<!-- PAGE 1 -->", "<!-- PAGE 2 -->"]);
        assert!(matches!(doc.elements[0].kind, ElementKind::RawBlock));
    }

    #[test]
    fn uses_element_page_numbers_when_present() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("Page three text.").with_page(3));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(paragraph("Page seven text.").with_page(7));

        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::RawBlock))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(texts, vec!["<!-- PAGE 3 -->", "<!-- PAGE 7 -->"]);
    }

    #[test]
    fn trailing_page_break_gets_no_dangling_marker() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("Only page text."));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));

        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);

        let raw_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::RawBlock))
            .count();
        assert_eq!(raw_count, 1);
    }

    #[test]
    fn custom_format_with_repeated_placeholder() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("Page 2 of document (page 2)"));
        doc.push_element(paragraph("Body text."));

        inject_page_marker_elements(&mut doc, "Page {page_num} of document (page {page_num})");

        assert!(matches!(doc.elements[0].kind, ElementKind::RawBlock));
        assert!(matches!(doc.elements[1].kind, ElementKind::Paragraph));
    }

    #[test]
    fn empty_document_is_untouched() {
        let mut doc = InternalDocument::new("pdf");
        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn non_marker_paragraphs_are_not_converted() {
        let mut doc = InternalDocument::new("pdf");
        doc.push_element(paragraph("This mentions <!-- PAGE 1 --> inline but is prose."));

        inject_page_marker_elements(&mut doc, DEFAULT_FORMAT);

        // No marker paragraph and no page breaks: page-1 marker is synthesized,
        // prose paragraph stays a paragraph.
        assert!(matches!(doc.elements[0].kind, ElementKind::RawBlock));
        assert!(matches!(doc.elements[1].kind, ElementKind::Paragraph));
    }
}
