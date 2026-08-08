//! Regression tests for a batch of ODT (OpenDocument Text) extraction defects:
//!
//! - #93  Deeply nested inline text (spans nested inside spans) was truncated.
//! - #94  `text:a` wrapping a styled `text:span` dropped the URL and/or text.
//! - #100 Comments (`office:annotation`), indexes, and page-anchored frames were
//!   dropped entirely.
//! - #104 Every list was hardcoded unordered regardless of its actual style.
//! - #112 A ZIP missing `content.xml` returned an empty success instead of
//!   failing loudly.
//! - #117 Footnotes and nested lists inside table cells were silently dropped;
//!   only the cell's leading plain text survived.
//!
//! Each test builds a minimal in-memory ODF ZIP rather than depending on the
//! `test_documents/odt` Git-LFS submodule, so these run unconditionally.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::core::config::ExtractionConfig;
use xberg::types::document_structure::{AnnotationKind, NodeContent};
use xberg::types::uri::UriKind;
use zip::write::{FileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

const ODT_MIME: &str = "application/vnd.oasis.opendocument.text";

/// Namespace declarations shared by every `content.xml` fixture below.
const CONTENT_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
    xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xlink="http://www.w3.org/1999/xlink"
    xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#;

/// Build a minimal in-memory `.odt` ZIP. `automatic_styles` is placed inside
/// `office:automatic-styles`; `body` is placed inside `office:text`.
fn odt_bytes(automatic_styles: &str, body: &str) -> Vec<u8> {
    let content_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_NS}>
  <office:automatic-styles>{automatic_styles}</office:automatic-styles>
  <office:body>
    <office:text>{body}</office:text>
  </office:body>
</office:document-content>"#
    );

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        // The `mimetype` entry must be first and stored uncompressed per the ODF spec. ~keep
        zip.start_file("mimetype", stored).expect("write mimetype");
        zip.write_all(ODT_MIME.as_bytes()).expect("write mimetype body");

        let deflated = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("content.xml", deflated).expect("write content.xml");
        zip.write_all(content_xml.as_bytes()).expect("write content.xml body");
        zip.finish().expect("finish zip");
    }
    cursor.into_inner()
}

/// Build an in-memory `.odt` ZIP that has no `content.xml` at all (#112).
fn odt_bytes_without_content_xml() -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).expect("write mimetype");
        zip.write_all(ODT_MIME.as_bytes()).expect("write mimetype body");
        zip.finish().expect("finish zip");
    }
    cursor.into_inner()
}

fn document_structure_config() -> ExtractionConfig {
    ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn should_preserve_text_of_span_nested_inside_span_issue_93() {
    let styles = r#"<style:style style:name="Bold1" style:family="text">
        <style:text-properties fo:font-weight="bold"/>
    </style:style>"#;
    // concat! joins these at compile time with no inserted characters, so the
    // XML stays whitespace-exact while the source line stays under 120 cols.
    let body = concat!(
        r#"<text:p>Before <text:span text:style-name="Bold1">outer "#,
        r#"<text:span>inner</text:span> tail</text:span> after</text:p>"#
    );

    let bytes = odt_bytes(styles, body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &ExtractionConfig::default())
        .await
        .expect("extraction should succeed");

    assert_eq!(
        result.content.trim(),
        "Before outer inner tail after",
        "text nested two levels deep inside a span must not be truncated"
    );
}

#[tokio::test]
async fn should_annotate_full_range_of_span_wrapping_a_nested_span_issue_93() {
    let styles = r#"<style:style style:name="Bold1" style:family="text">
        <style:text-properties fo:font-weight="bold"/>
    </style:style>"#;
    let body = r#"<text:p><text:span text:style-name="Bold1">outer <text:span>inner</text:span></text:span></text:p>"#;

    let bytes = odt_bytes(styles, body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    let paragraph = structure
        .nodes
        .iter()
        .find(|n| matches!(&n.content, NodeContent::Paragraph { text } if text == "outer inner"))
        .expect("expected a paragraph node with the full nested-span text");

    // "outer inner" is 11 bytes; the outer span covers the whole thing.
    assert!(
        paragraph
            .annotations
            .iter()
            .any(|a| a.kind == AnnotationKind::Bold && a.start == 0 && a.end == 11),
        "bold annotation should span the outer span's full flattened text, annotations: {:?}",
        paragraph.annotations
    );
}

#[tokio::test]
async fn should_preserve_url_and_text_of_hyperlink_wrapping_styled_span_issue_94() {
    let styles = r#"<style:style style:name="Bold1" style:family="text">
        <style:text-properties fo:font-weight="bold"/>
    </style:style>"#;
    let body = concat!(
        r#"<text:p>Visit <text:a xlink:href="https://example.com/page">"#,
        r#"<text:span text:style-name="Bold1">our site</text:span></text:a> today</text:p>"#
    );

    let bytes = odt_bytes(styles, body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");

    assert_eq!(
        result.content.trim(),
        "Visit our site today",
        "text:a wrapping a styled text:span must not drop the link text"
    );

    let uris = result.uris.expect("expected uris to be populated");
    assert!(
        uris.iter().any(|u| u.url == "https://example.com/page"
            && u.label.as_deref() == Some("our site")
            && u.kind == UriKind::Hyperlink),
        "expected the wrapped link's URL and label to be extracted, got: {:?}",
        uris
    );

    let structure = result.document.expect("document structure should be present");
    let paragraph = structure
        .nodes
        .iter()
        .find(|n| matches!(&n.content, NodeContent::Paragraph { text } if text == "Visit our site today"))
        .expect("expected the paragraph node");
    assert!(
        paragraph.annotations.iter().any(|a| matches!(
            &a.kind,
            AnnotationKind::Link { url, .. } if url == "https://example.com/page"
        ) && a.start == 6
            && a.end == 14),
        "expected a Link annotation over 'our site' (bytes 6..14), annotations: {:?}",
        paragraph.annotations
    );
    assert!(
        paragraph
            .annotations
            .iter()
            .any(|a| a.kind == AnnotationKind::Bold && a.start == 6 && a.end == 14),
        "the span's own Bold styling should survive being wrapped in text:a, annotations: {:?}",
        paragraph.annotations
    );
}

#[tokio::test]
async fn should_extract_comment_as_footnote_style_node_issue_100() {
    let body = r#"<text:p>Please review this figure.<office:annotation office:name="c1">
        <dc:creator>Alice</dc:creator>
        <text:p>This needs revision.</text:p>
    </office:annotation></text:p>"#;

    let bytes = odt_bytes("", body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    let has_comment = structure
        .nodes
        .iter()
        .any(|n| matches!(&n.content, NodeContent::Footnote { text } if text == "This needs revision."));
    assert!(
        has_comment,
        "office:annotation body should surface as a Footnote-shaped node; nodes: {:?}",
        structure.nodes
    );
}

#[tokio::test]
async fn should_extract_table_of_contents_index_body_issue_100() {
    let body = r#"<text:table-of-content>
        <text:index-body>
            <text:index-title><text:p>Table of Contents</text:p></text:index-title>
            <text:p>Chapter One 1</text:p>
        </text:index-body>
    </text:table-of-content>"#;

    let bytes = odt_bytes("", body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &ExtractionConfig::default())
        .await
        .expect("extraction should succeed");

    assert!(
        result.content.contains("Chapter One 1"),
        "index-body content should be extracted instead of dropped; got: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_extract_page_anchored_frame_not_wrapped_in_a_paragraph_issue_100() {
    let body = r#"<text:p>Body text.</text:p>
    <draw:frame draw:name="Logo" text:anchor-type="page">
        <draw:image xlink:href="Pictures/logo.png"/>
    </draw:frame>"#;

    let bytes = odt_bytes("", body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    let has_frame_image = structure.nodes.iter().any(|n| {
        matches!(
            &n.content,
            NodeContent::Image { src: Some(src), .. } if src == "Pictures/logo.png"
        )
    });
    assert!(
        has_frame_image,
        "a draw:frame anchored to the page (not inside a text:p) must not be dropped; nodes: {:?}",
        structure.nodes
    );
}

#[tokio::test]
async fn should_mark_numbered_list_ordered_and_bulleted_list_unordered_issue_104() {
    let styles = r#"<text:list-style style:name="L1">
        <text:list-level-style-number text:level="1"/>
    </text:list-style>
    <text:list-style style:name="L2">
        <text:list-level-style-bullet text:level="1"/>
    </text:list-style>"#;
    let body = r#"<text:list text:style-name="L1">
        <text:list-item><text:p>One</text:p></text:list-item>
        <text:list-item><text:p>Two</text:p></text:list-item>
    </text:list>
    <text:list text:style-name="L2">
        <text:list-item><text:p>Alpha</text:p></text:list-item>
    </text:list>"#;

    let bytes = odt_bytes(styles, body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    let has_ordered_list = structure
        .nodes
        .iter()
        .any(|n| matches!(&n.content, NodeContent::List { ordered: true }));
    let has_unordered_list = structure
        .nodes
        .iter()
        .any(|n| matches!(&n.content, NodeContent::List { ordered: false }));

    assert!(
        has_ordered_list,
        "a list styled with text:list-level-style-number must be marked ordered; nodes: {:?}",
        structure.nodes
    );
    assert!(
        has_unordered_list,
        "a list styled with text:list-level-style-bullet must be marked unordered; nodes: {:?}",
        structure.nodes
    );
}

#[tokio::test]
async fn should_resolve_named_list_style_declared_only_in_styles_xml_issue_104() {
    let content_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_NS}>
  <office:body>
    <office:text>
        <text:list text:style-name="L9">
            <text:list-item><text:p>One</text:p></text:list-item>
        </text:list>
    </office:text>
  </office:body>
</office:document-content>"#
    );
    let styles_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles {CONTENT_NS}>
  <office:styles>
    <text:list-style style:name="L9">
        <text:list-level-style-number text:level="1"/>
    </text:list-style>
  </office:styles>
</office:document-styles>"#
    );

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).expect("write mimetype");
        zip.write_all(ODT_MIME.as_bytes()).expect("write mimetype body");

        let deflated = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("content.xml", deflated).expect("write content.xml");
        zip.write_all(content_xml.as_bytes()).expect("write content.xml body");
        zip.start_file("styles.xml", deflated).expect("write styles.xml");
        zip.write_all(styles_xml.as_bytes()).expect("write styles.xml body");
        zip.finish().expect("finish zip");
    }
    let bytes = cursor.into_inner();

    let result = extract_bytes_document(&bytes, ODT_MIME, &document_structure_config())
        .await
        .expect("extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    assert!(
        structure
            .nodes
            .iter()
            .any(|n| matches!(&n.content, NodeContent::List { ordered: true })),
        "a list style declared only in styles.xml's office:styles must still resolve as ordered; nodes: {:?}",
        structure.nodes
    );
}

#[tokio::test]
async fn should_fail_loudly_when_content_xml_is_missing_issue_112() {
    let bytes = odt_bytes_without_content_xml();

    let err = extract_bytes_document(&bytes, ODT_MIME, &ExtractionConfig::default())
        .await
        .expect_err("extraction of an ODF ZIP with no content.xml must fail, not return an empty success");

    let message = err.to_string();
    assert!(
        message.contains("content.xml"),
        "error message should name the missing member; got: {message}"
    );
}

#[tokio::test]
async fn should_preserve_footnote_inside_table_cell_issue_117() {
    let body = r#"<table:table>
        <table:table-row>
            <table:table-cell><text:p>Cell with note<text:note text:note-class="footnote">
                <text:note-citation>1</text:note-citation>
                <text:note-body><text:p>Cell footnote body</text:p></text:note-body>
            </text:note></text:p></table:table-cell>
        </table:table-row>
    </table:table>"#;

    let bytes = odt_bytes("", body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &ExtractionConfig::default())
        .await
        .expect("extraction should succeed");

    assert_eq!(result.tables.len(), 1, "expected exactly one table");
    assert_eq!(
        result.tables[0].cells[0][0], "Cell with note\n[footnote 1: Cell footnote body]",
        "a footnote inside a table cell must be flattened into the cell text, not dropped"
    );
}

#[tokio::test]
async fn should_preserve_nested_list_inside_table_cell_issue_117() {
    // Deliberately single-line: whitespace between sibling elements is a
    // significant text node to the ODF walker (e.g. inside a table cell,
    // where indentation would otherwise leak into the flattened cell text).
    let body = concat!(
        r#"<table:table><table:table-row><table:table-cell><text:list>"#,
        r#"<text:list-item><text:p>Item A</text:p></text:list-item>"#,
        r#"<text:list-item><text:p>Item B</text:p></text:list-item>"#,
        r#"</text:list></table:table-cell></table:table-row></table:table>"#
    );

    let bytes = odt_bytes("", body);
    let result = extract_bytes_document(&bytes, ODT_MIME, &ExtractionConfig::default())
        .await
        .expect("extraction should succeed");

    assert_eq!(result.tables.len(), 1, "expected exactly one table");
    assert_eq!(
        result.tables[0].cells[0][0], "Item A\nItem B",
        "a nested list inside a table cell must be flattened into the cell text, not dropped"
    );
}
