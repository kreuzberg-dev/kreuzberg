//! Render an `InternalDocument` to plain text.
//!
//! Emits text only, with no formatting. Double newlines separate blocks.
//! Annotations are stripped. Tables are rendered as space-separated columns.

use crate::types::document_structure::ContentLayer;
use crate::types::internal::{ElementKind, InternalDocument};

use super::common::{get_admonition_kind, get_admonition_title, parse_metadata_entries, render_table_plain};

/// Render an `InternalDocument` to plain text.
pub(crate) fn render_plain(doc: &InternalDocument) -> String {
    let mut out = String::with_capacity(doc.elements.len() * 80);
    let mut last_heading_depth: Option<u16> = None;

    for elem in &doc.elements {
        if elem.layer != ContentLayer::Body {
            continue;
        }

        if elem.kind.is_container_start() || elem.kind.is_container_end() {
            continue;
        }

        match elem.kind {
            ElementKind::Title | ElementKind::Heading { .. } | ElementKind::Paragraph => {
                if !elem.text.is_empty() {
                    if matches!(elem.kind, ElementKind::Heading { .. }) {
                        if let Some(last_depth) = last_heading_depth
                            && (last_depth == 0 || last_depth == 1)
                            && last_depth == elem.depth
                            && !out.is_empty()
                            && !out.ends_with("\n\n")
                        {
                            out.push('\n');
                        }
                        last_heading_depth = Some(elem.depth);
                    }

                    if matches!(elem.kind, ElementKind::Paragraph)
                        || (matches!(elem.kind, ElementKind::Heading { .. }) && elem.depth > 0)
                    {
                        let indent = "  ".repeat(elem.depth as usize);
                        out.push_str(&indent);
                    }

                    if let (true, Some(attrs)) = (matches!(elem.kind, ElementKind::Heading { .. }), &elem.attributes) {
                        out.push_str(&elem.text);
                        let mut filtered_attrs: Vec<_> = attrs
                            .iter()
                            .filter(|(k, v)| !k.starts_with("xmlns") && !v.is_empty())
                            .collect();
                        filtered_attrs.sort_by_key(|(k, _)| k.as_str());
                        let formatted_attrs: Vec<String> =
                            filtered_attrs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                        if !formatted_attrs.is_empty() {
                            out.push_str(" (");
                            out.push_str(&formatted_attrs.join(", "));
                            out.push(')');
                        }
                    } else {
                        out.push_str(&elem.text);
                    }

                    if matches!(elem.kind, ElementKind::Heading { .. }) {
                        out.push('\n');
                    } else {
                        out.push_str("\n\n");
                    }
                }
            }
            ElementKind::ListItem { .. } => {
                out.push_str(&elem.text);
                out.push('\n');
            }
            ElementKind::Code => {
                out.push_str(&elem.text);
                if !elem.text.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            ElementKind::Formula => {
                out.push_str(&elem.text);
                out.push_str("\n\n");
            }
            ElementKind::Table { table_index } => {
                if let Some(table) = doc.tables.get(table_index as usize) {
                    let table_str = if !elem.text.is_empty() {
                        elem.text.clone()
                    } else if !table.cells.is_empty() {
                        render_table_plain(&table.cells)
                    } else {
                        table.markdown.clone()
                    };
                    if !table_str.trim().is_empty() {
                        out.push_str(&table_str);
                        out.push('\n');
                    }
                }
            }
            ElementKind::Image { image_index } => {
                if let Some(img) = doc.images.get(image_index as usize) {
                    if let Some(ref desc) = img.description
                        && !desc.is_empty()
                    {
                        out.push_str("[Image: ");
                        out.push_str(desc);
                        out.push_str("]\n\n");
                    }

                    if elem.should_render_image_ocr()
                        && let Some(ocr_result) = &img.ocr_result
                        && !ocr_result.content.is_empty()
                    {
                        out.push_str(&ocr_result.content);
                        out.push_str("\n\n");
                    }
                }
            }
            ElementKind::FootnoteRef => {}
            ElementKind::FootnoteDefinition => {}
            ElementKind::Citation => {
                if !elem.text.is_empty() {
                    out.push_str(&elem.text);
                    out.push_str("\n\n");
                }
            }
            ElementKind::PageBreak => {
                out.push('\n');
            }
            ElementKind::Slide { .. } => {
                if !elem.text.is_empty() {
                    out.push_str(&elem.text);
                    out.push_str("\n\n");
                }
            }
            ElementKind::DefinitionTerm => {
                out.push_str(&elem.text);
                out.push_str(": ");
            }
            ElementKind::DefinitionDescription => {
                out.push_str(&elem.text);
                out.push_str("\n\n");
            }
            ElementKind::Admonition => {
                let title = get_admonition_title(elem);
                if let Some(t) = title {
                    out.push_str(t);
                } else {
                    out.push_str(get_admonition_kind(elem));
                }
                out.push_str("\n\n");
                if !elem.text.is_empty() {
                    out.push_str(&elem.text);
                    out.push_str("\n\n");
                }
            }
            ElementKind::RawBlock => {
                out.push_str(&elem.text);
                if !elem.text.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            ElementKind::MetadataBlock => {
                let entries = parse_metadata_entries(&elem.text);
                if !entries.is_empty() {
                    for (key, value) in &entries {
                        out.push_str(key);
                        out.push_str(": ");
                        out.push_str(value);
                        out.push('\n');
                    }
                    out.push('\n');
                } else if !elem.text.is_empty() {
                    out.push_str(&elem.text);
                    if !elem.text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push('\n');
                }
            }
            ElementKind::OcrText { .. } => {
                if !elem.text.is_empty() {
                    out.push_str(&elem.text);
                    out.push_str("\n\n");
                }
            }
            ElementKind::ListStart { .. }
            | ElementKind::ListEnd
            | ElementKind::QuoteStart
            | ElementKind::QuoteEnd
            | ElementKind::GroupStart
            | ElementKind::GroupEnd => {}
        }
    }

    let has_footnotes = doc
        .elements
        .iter()
        .any(|e| e.kind == ElementKind::FootnoteDefinition && e.layer == ContentLayer::Footnote);
    if has_footnotes {
        out.push('\n');
        for elem in &doc.elements {
            if elem.kind == ElementKind::FootnoteDefinition && elem.layer == ContentLayer::Footnote {
                out.push_str(&elem.text);
                out.push_str("\n\n");
            }
        }
    }

    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document_structure::ContentLayer;
    use crate::types::internal_builder::InternalDocumentBuilder;

    #[test]
    fn test_render_plain_title() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_title("My Document", None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert_eq!(out, "My Document");
    }

    #[test]
    fn test_render_plain_heading() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_heading(2, "Section", None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert_eq!(out, "  Section");
    }

    #[test]
    fn test_render_plain_paragraph() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Hello world.", vec![], None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert_eq!(out, "Hello world.");
    }

    #[test]
    fn test_render_plain_list_items() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_list_item("Alpha", false, vec![], None, None);
        b.push_list_item("Beta", false, vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Alpha\n"), "got: {}", out);
        assert!(out.contains("Beta"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_ordered_list() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(true);
        b.push_list_item("First", true, vec![], None, None);
        b.push_list_item("Second", true, vec![], None, None);
        b.end_list();
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("First"), "got: {}", out);
        assert!(out.contains("Second"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_code_block() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_code("fn main() {}", Some("rust"), None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("fn main() {}"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_formula() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_formula("E = mc^2", None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("E = mc^2"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_table() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];
        b.push_table_from_cells(&cells, None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Name Age"), "got: {}", out);
        assert!(out.contains("Alice 30"), "got: {}", out);
    }

    #[test]
    fn should_use_explicit_plain_text_for_table_when_present() {
        let mut builder = InternalDocumentBuilder::new("test");
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];
        let table_index = builder.push_table_from_cells(&cells, None, None);
        builder.set_text(table_index, "Name: Alice\nAge: 30");

        let output = render_plain(&builder.build());

        assert_eq!(output, "Name: Alice\nAge: 30");
    }

    #[test]
    fn test_render_plain_image() {
        let mut b = InternalDocumentBuilder::new("test");
        let image = crate::types::ExtractedImage {
            data: bytes::Bytes::new(),
            format: std::borrow::Cow::Borrowed("png"),
            image_index: 0,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: Some("A nice photo".to_string()),
            ocr_result: None,
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };
        b.push_image(Some("A nice photo"), image, None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("[Image: A nice photo]"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_page_break() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Before", vec![], None, None);
        b.push_page_break();
        b.push_paragraph("After", vec![], None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Before"), "got: {}", out);
        assert!(out.contains("After"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_slide() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_slide(1, Some("Slide Title"), None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Slide Title"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_definition_term_and_description() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_definition_term("Rust", None);
        b.push_definition_description("A systems language", None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Rust: "), "got: {}", out);
        assert!(out.contains("A systems language"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_admonition_with_title() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_admonition("warning", Some("Be careful"), None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Be careful"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_admonition_without_title() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_admonition("note", None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("note"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_raw_block() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_raw_block("tex", "\\LaTeX{}", None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("\\LaTeX{}"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_metadata_block() {
        let mut b = InternalDocumentBuilder::new("test");
        let entries = vec![("Author".to_string(), "Alice".to_string())];
        b.push_metadata_block(&entries, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Author: Alice"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_empty_document() {
        let b = InternalDocumentBuilder::new("test");
        let doc = b.build();
        let out = render_plain(&doc);
        assert_eq!(out, "");
    }

    #[test]
    fn test_render_plain_strips_annotations() {
        use crate::types::document_structure::{AnnotationKind, TextAnnotation};
        let mut b = InternalDocumentBuilder::new("test");
        let ann = vec![TextAnnotation {
            start: 0,
            end: 5,
            kind: AnnotationKind::Bold,
        }];
        b.push_paragraph("Hello world", ann, None, None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert_eq!(out, "Hello world");
    }

    #[test]
    fn test_render_plain_blockquote_content() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_quote_start();
        b.push_paragraph("Quoted text.", vec![], None, None);
        b.push_quote_end();
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Quoted text."), "got: {}", out);
    }

    #[test]
    fn test_render_plain_nested_list() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_list_item("Outer", false, vec![], None, None);
        b.push_list(false);
        b.push_list_item("Inner", false, vec![], None, None);
        b.end_list();
        b.end_list();
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Outer"), "got: {}", out);
        assert!(out.contains("Inner"), "got: {}", out);
    }

    #[test]
    fn test_render_plain_footnote_definitions_at_end() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Main text", vec![], None, None);
        b.push_footnote_ref("1", "fn1", None);
        let def = b.push_footnote_definition("A note.", "fn1", None);
        b.set_layer(def, ContentLayer::Footnote);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Main text"), "got: {}", out);
        assert!(out.contains("A note."), "got: {}", out);
    }

    #[test]
    fn test_render_plain_citation() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_citation("Smith 2024", "smith2024", None);
        let doc = b.build();
        let out = render_plain(&doc);
        assert!(out.contains("Smith 2024"), "got: {}", out);
    }
}
