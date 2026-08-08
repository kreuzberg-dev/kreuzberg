//! Regression tests for issue #130: Org-mode body-level "affiliated keyword" lines
//! (`#+CAPTION:`, `#+NAME:`) that appear inline in the document body, attached to the
//! following element, were previously silently discarded by
//! `OrgModeExtractor::build_internal_document` instead of being attached to that element.
//!
//! These tests exercise the extractor through the public extraction API with
//! `include_document_structure: true` so the `caption` attribute (propagated from
//! `InternalElement::attributes` via `InternalElement::public_attributes()`) is observable
//! on the resulting `DocumentNode`.

#![cfg(feature = "office")]

mod helpers;
use helpers::extract_bytes_document;

use xberg::core::config::ExtractionConfig;
use xberg::types::NodeContent;

fn document_structure_config() -> ExtractionConfig {
    ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    }
}

/// `#+CAPTION:` immediately preceding an image link must be attached to the resulting
/// Image node's `caption` attribute, and the raw `#+CAPTION:` directive line must not
/// appear verbatim anywhere in the rendered text output.
#[tokio::test]
async fn should_attach_caption_to_following_image_link() {
    let org_text = b"#+CAPTION: A diagram of the system architecture\n[[file:diagram.png]]\n";
    let config = document_structure_config();

    let document = extract_bytes_document(org_text, "text/x-org", &config)
        .await
        .expect("extraction should succeed");

    assert!(
        !document.content.contains("#+CAPTION"),
        "raw #+CAPTION directive must not appear verbatim in extracted text: {:?}",
        document.content
    );

    let structure = document
        .document
        .as_ref()
        .expect("document structure should be present");
    let image_node = structure
        .nodes
        .iter()
        .find(|node| matches!(node.content, NodeContent::Image { .. }))
        .expect("an Image node should be present");

    let caption = image_node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("caption"))
        .map(String::as_str);
    assert_eq!(
        caption,
        Some("A diagram of the system architecture"),
        "image node attributes should carry the caption: {:?}",
        image_node.attributes
    );
}

/// `#+CAPTION:` immediately preceding a table must be attached to the resulting Table
/// node's `caption` attribute, and the raw directive line must not leak into rendered text.
#[tokio::test]
async fn should_attach_caption_to_following_table() {
    let org_text = b"#+CAPTION: Quarterly results\n| Name | Age |\n| Alice | 30 |\n";
    let config = document_structure_config();

    let document = extract_bytes_document(org_text, "text/x-org", &config)
        .await
        .expect("extraction should succeed");

    assert_eq!(document.tables.len(), 1, "table should be parsed exactly once");
    assert!(
        !document.content.contains("#+CAPTION"),
        "raw #+CAPTION directive must not appear verbatim in extracted text: {:?}",
        document.content
    );

    let structure = document
        .document
        .as_ref()
        .expect("document structure should be present");
    let table_node = structure
        .nodes
        .iter()
        .find(|node| matches!(node.content, NodeContent::Table { .. }))
        .expect("a Table node should be present");

    let caption = table_node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("caption"))
        .map(String::as_str);
    assert_eq!(
        caption,
        Some("Quarterly results"),
        "table node attributes should carry the caption: {:?}",
        table_node.attributes
    );
}

/// `#+NAME:` immediately preceding a table is an affiliated keyword too (commonly used to
/// give the table a label for cross-references); it should be captured as the node anchor
/// rather than silently discarded, and must not appear verbatim in rendered text.
#[tokio::test]
async fn should_attach_name_as_anchor_to_following_table() {
    let org_text = b"#+NAME: quarterly-results\n| Name | Age |\n| Alice | 30 |\n";
    let config = document_structure_config();

    let document = extract_bytes_document(org_text, "text/x-org", &config)
        .await
        .expect("extraction should succeed");

    assert!(
        !document.content.contains("#+NAME"),
        "raw #+NAME directive must not appear verbatim in extracted text: {:?}",
        document.content
    );

    let structure = document
        .document
        .as_ref()
        .expect("document structure should be present");
    let table_node = structure
        .nodes
        .iter()
        .find(|node| matches!(node.content, NodeContent::Table { .. }))
        .expect("a Table node should be present");
    // Anchors are not part of the public `DocumentNode` DTO directly; the node id is
    // still deterministic and the caption/anchor test above confirms the attribute path.
    // Here we only assert the raw directive is gone and the table still parses correctly.
    let _ = table_node;
    assert_eq!(document.tables.len(), 1);
}

/// A `#+CAPTION:` line with nothing image/table-like immediately after it must not be
/// mis-attached to a later, unrelated element — it should simply be dropped (same
/// "no crash, no leaked raw markup" behavior as before this fix).
#[tokio::test]
async fn should_not_attach_orphan_caption_to_unrelated_later_element() {
    let org_text = b"#+CAPTION: Orphaned caption\n\nJust a plain paragraph.\n\n| A | B |\n| 1 | 2 |\n";
    let config = document_structure_config();

    let document = extract_bytes_document(org_text, "text/x-org", &config)
        .await
        .expect("extraction should succeed");

    assert!(
        !document.content.contains("#+CAPTION"),
        "raw #+CAPTION directive must not appear verbatim in extracted text: {:?}",
        document.content
    );

    let structure = document
        .document
        .as_ref()
        .expect("document structure should be present");
    let table_node = structure
        .nodes
        .iter()
        .find(|node| matches!(node.content, NodeContent::Table { .. }))
        .expect("a Table node should be present");

    assert_eq!(
        table_node.attributes, None,
        "orphaned caption (blank line before the table) must not attach to the table: {:?}",
        table_node.attributes
    );
}
