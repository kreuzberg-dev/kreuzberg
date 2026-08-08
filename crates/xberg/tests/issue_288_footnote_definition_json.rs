//! Regression test for defect #288 — footnote definitions were dropped from
//! `render_json` output.
//!
//! `rendering/json.rs` filters the per-element loop with `is_body_element`, which
//! only lets `ContentLayer::Body` elements through. Real extractors (docx, hwpx,
//! odt) push a footnote definition and then explicitly move it to
//! `ContentLayer::Footnote` via `set_layer`, since a definition is document
//! furniture, not body flow. That filter is correct on its own terms, but it meant
//! the `ElementKind::FootnoteDefinition` match arm — which already builds a
//! `JsonNode::FootnoteDefinition` — was unreachable for every definition a real
//! extractor produces: the reference's `[^n]` marker survived in body text and as
//! a `footnote_ref` node, but the definition it points at vanished, leaving a
//! dangling marker with no way to resolve it.
//!
//! Run with:
//!   cargo test -p xberg --test issue_288_footnote_definition_json

use serde_json::{Value, json};
use xberg::rendering::render_json;
use xberg::types::ContentLayer;
use xberg::types::internal::InternalDocument;
use xberg::types::internal_builder::InternalDocumentBuilder;

fn render_body(document: InternalDocument) -> Value {
    let rendered = render_json(&document);
    let parsed: Value = serde_json::from_str(&rendered).expect("render_json must emit valid JSON");
    parsed["body"].clone()
}

/// Mirrors the real extractor pattern (see `extractors/docx.rs`): the reference
/// marker is embedded directly in the paragraph text (as `[^1]`), a structured
/// `FootnoteRef` element is pushed alongside it, and the definition is pushed and
/// then moved onto `ContentLayer::Footnote` — furniture, not body flow.
#[test]
fn should_emit_footnote_definition_reachable_from_a_body_reference() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_paragraph("See the note.[^1]", vec![], None, None);
    builder.push_footnote_ref("1", "fn1", None);
    let def_idx = builder.push_footnote_definition("Detailed footnote text.", "fn1", None);
    builder.set_layer(def_idx, ContentLayer::Footnote);

    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([
            { "type": "paragraph", "text": "See the note.[^1]" },
            { "type": "footnote_ref", "number": 1, "id": "fn1" },
            { "type": "footnote_definition", "text": "Detailed footnote text.", "id": "fn1" },
        ]),
        "the [^1] marker in body text must resolve via a footnote_definition node sharing id \"fn1\""
    );
}

/// A definition on `ContentLayer::Footnote` is furniture, not body flow: it must
/// not be interleaved into whichever section happened to be open when its
/// reference occurred. It still has to reach the consumer, so it is appended once,
/// at the very end of the document, outside every section.
#[test]
fn should_emit_footnote_definition_outside_the_enclosing_section() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_heading(1, "Chapter 1", None, None);
    builder.push_paragraph("See the note.[^1]", vec![], None, None);
    builder.push_footnote_ref("1", "fn1", None);
    let def_idx = builder.push_footnote_definition("Detailed footnote text.", "fn1", None);
    builder.set_layer(def_idx, ContentLayer::Footnote);

    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([
            {
                "type": "section",
                "heading": "Chapter 1",
                "level": 1,
                "body": [
                    { "type": "paragraph", "text": "See the note.[^1]" },
                    { "type": "footnote_ref", "number": 1, "id": "fn1" },
                ],
            },
            { "type": "footnote_definition", "text": "Detailed footnote text.", "id": "fn1" },
        ]),
        "the footnote_definition must land at the top level, not nested inside Chapter 1's section"
    );
}

/// An unreferenced definition is still authored content and must not be silently
/// dropped, matching the `FootnoteCollector` orphan handling used by other
/// renderers (see xberg-io/xberg#68).
#[test]
fn should_emit_an_unreferenced_footnote_definition() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_paragraph("No marker points here.", vec![], None, None);
    let def_idx = builder.push_footnote_definition("Orphaned footnote text.", "fn9", None);
    builder.set_layer(def_idx, ContentLayer::Footnote);

    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([
            { "type": "paragraph", "text": "No marker points here." },
            { "type": "footnote_definition", "text": "Orphaned footnote text.", "id": "fn9" },
        ]),
        "an unreferenced footnote definition must still reach the JSON output"
    );
}

/// Two footnote definitions must be emitted in document order.
#[test]
fn should_preserve_document_order_across_multiple_footnote_definitions() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_paragraph("First note.[^1] Second note.[^2]", vec![], None, None);
    builder.push_footnote_ref("1", "fn1", None);
    builder.push_footnote_ref("2", "fn2", None);
    let def1 = builder.push_footnote_definition("First footnote body.", "fn1", None);
    builder.set_layer(def1, ContentLayer::Footnote);
    let def2 = builder.push_footnote_definition("Second footnote body.", "fn2", None);
    builder.set_layer(def2, ContentLayer::Footnote);

    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([
            { "type": "paragraph", "text": "First note.[^1] Second note.[^2]" },
            { "type": "footnote_ref", "number": 1, "id": "fn1" },
            { "type": "footnote_ref", "number": 2, "id": "fn2" },
            { "type": "footnote_definition", "text": "First footnote body.", "id": "fn1" },
            { "type": "footnote_definition", "text": "Second footnote body.", "id": "fn2" },
        ]),
        "footnote definitions must be emitted in document order"
    );
}
