//! Regression tests for ODP (OpenDocument Presentation) extraction defects:
//!
//! - #95  Speaker notes (`presentation:notes`) were never extracted at all.
//! - #116 Shapes other than `draw:frame > draw:text-box` (e.g.
//!   `draw:custom-shape`) and `draw:g` groups were invisible to the walker:
//!   only a bare `draw:frame` holding a `text-box`, `table`, or `image` was
//!   ever visited.
//!
//! Each test builds a minimal in-memory `.odp` ZIP rather than depending on
//! the `test_documents/odp` Git-LFS submodule, so these run unconditionally.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::core::config::ExtractionConfig;
use xberg::types::document_structure::NodeContent;
use zip::write::{FileOptions, ZipWriter};

mod helpers;
use helpers::extract_bytes_document;

const ODP_MIME: &str = "application/vnd.oasis.opendocument.presentation";

/// Build a minimal in-memory `.odp` ZIP. `body_inner` is placed inside
/// `<office:presentation>`.
fn odp_bytes(body_inner: &str) -> Vec<u8> {
    let content_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0"
    xmlns:xlink="http://www.w3.org/1999/xlink">
  <office:body>
    <office:presentation>{body_inner}</office:presentation>
  </office:body>
</office:document-content>"#
    );

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        // The `mimetype` entry must be first and stored uncompressed per the ODF spec. ~keep
        zip.start_file("mimetype", stored).expect("write mimetype");
        zip.write_all(ODP_MIME.as_bytes()).expect("write mimetype body");

        let deflated = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("content.xml", deflated).expect("write content.xml");
        zip.write_all(content_xml.as_bytes()).expect("write content.xml body");
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
async fn should_extract_speaker_notes_as_a_distinct_raw_block_issue_95() {
    let notes_page = concat!(
        r#"<draw:page draw:name="Intro">"#,
        r#"<draw:frame><draw:text-box><text:p>Slide body text</text:p></draw:text-box></draw:frame>"#,
        r#"<presentation:notes><draw:frame><draw:text-box>"#,
        r#"<text:p>Remember to smile</text:p></draw:text-box></draw:frame></presentation:notes>"#,
        r#"</draw:page>"#
    );
    let bytes = odp_bytes(notes_page);

    let result = extract_bytes_document(&bytes, ODP_MIME, &document_structure_config())
        .await
        .expect("ODP extraction should succeed");
    let structure = result.document.expect("document structure should be present");

    let notes_block = structure.nodes.iter().find(|n| {
        matches!(
            &n.content,
            NodeContent::RawBlock { format, .. } if format == "odp-speaker-notes"
        )
    });
    let Some(notes_block) = notes_block else {
        panic!("expected an odp-speaker-notes raw block; nodes: {:?}", structure.nodes);
    };
    let NodeContent::RawBlock { content, .. } = &notes_block.content else {
        unreachable!()
    };
    assert_eq!(content.as_str(), "Remember to smile");

    let has_slide_body = structure
        .nodes
        .iter()
        .any(|n| matches!(&n.content, NodeContent::Paragraph { text } if text == "Slide body text"));
    assert!(
        has_slide_body,
        "speaker notes must not replace or merge into the slide's own body text"
    );

    let notes_leaked_into_body = structure
        .nodes
        .iter()
        .any(|n| matches!(&n.content, NodeContent::Paragraph { text } if text == "Remember to smile"));
    assert!(
        !notes_leaked_into_body,
        "speaker notes must stay a distinct raw block, not a slide-body paragraph"
    );
}

#[tokio::test]
async fn should_extract_text_from_a_custom_shape_not_wrapped_in_a_text_box_issue_116() {
    // Real-world decks commonly carry title/body placeholder text directly
    // on a draw:custom-shape rather than inside a draw:text-box.
    let bytes = odp_bytes(
        r#"<draw:page draw:name="Title">
            <draw:frame><draw:custom-shape><text:p>Quarterly Review</text:p></draw:custom-shape></draw:frame>
        </draw:page>"#,
    );

    let result = extract_bytes_document(&bytes, ODP_MIME, &ExtractionConfig::default())
        .await
        .expect("ODP extraction should succeed");

    assert!(
        result.content.contains("Quarterly Review"),
        "text directly inside a draw:custom-shape must be extracted; got: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_extract_shapes_and_tables_nested_inside_a_draw_g_group_issue_116() {
    let bytes = odp_bytes(
        r#"<draw:page draw:name="Grouped">
            <draw:g>
                <draw:frame><draw:text-box><text:p>Grouped text box</text:p></draw:text-box></draw:frame>
                <draw:custom-shape><text:p>Grouped shape text</text:p></draw:custom-shape>
                <draw:g>
                    <draw:frame><draw:text-box><text:p>Nested group text</text:p></draw:text-box></draw:frame>
                </draw:g>
            </draw:g>
        </draw:page>"#,
    );

    let result = extract_bytes_document(&bytes, ODP_MIME, &ExtractionConfig::default())
        .await
        .expect("ODP extraction should succeed");

    assert!(
        result.content.contains("Grouped text box"),
        "a frame nested inside a draw:g group must be extracted; got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("Grouped shape text"),
        "a shape nested inside a draw:g group must be extracted; got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("Nested group text"),
        "a frame nested inside a draw:g group nested inside another group must be extracted; got: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_warn_when_an_embedded_non_formula_object_cannot_be_extracted_issue_116() {
    let bytes = odp_bytes(
        r#"<draw:page draw:name="Embed">
            <draw:frame><draw:object xlink:href="./Object 1"/></draw:frame>
        </draw:page>"#,
    );

    let result = extract_bytes_document(&bytes, ODP_MIME, &ExtractionConfig::default())
        .await
        .expect("ODP extraction should succeed even when an embedded object can't be converted to text");

    assert!(
        result
            .processing_warnings
            .iter()
            .any(|w| w.source == "odp" && w.message.contains("Object 1")),
        "an unresolvable embedded object must produce a processing warning naming it, got: {:?}",
        result.processing_warnings
    );
}
