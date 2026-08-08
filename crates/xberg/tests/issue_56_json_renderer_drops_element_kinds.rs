//! Regression test for defect #56 — the JSON renderer silently discarded content.
//!
//! `rendering/json.rs` matched ten content-bearing `ElementKind`s in a catch-all
//! arm that did nothing, so page breaks, footnotes, citations, slides, definition
//! lists, admonitions, raw blocks, and metadata blocks vanished from `render_json`
//! output even though every other renderer emits them.
//!
//! Run with:
//!   cargo test -p xberg --features office --test issue_56_json_renderer_drops_element_kinds

use serde_json::{Value, json};
use xberg::rendering::render_json;
use xberg::types::internal_builder::InternalDocumentBuilder;

/// Build a document containing exactly the ten previously-dropped element kinds,
/// in the order they are asserted below.
fn document_with_previously_dropped_kinds() -> xberg::types::internal::InternalDocument {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_page_break();
    builder.push_footnote_ref("1", "fn1", None);
    builder.push_footnote_definition("A footnote body.", "fn1", None);
    builder.push_citation("Smith 2024", "smith2024", None);
    builder.push_slide(3, Some("Third Slide"), None);
    builder.push_definition_term("Rust", None);
    builder.push_definition_description("A systems language", None);
    builder.push_admonition("warning", Some("Be careful"), None);
    builder.push_raw_block("tex", "\\LaTeX{}", None);
    builder.push_metadata_block(&[("Author".to_string(), "Alice".to_string())], None);
    builder.build()
}

fn render_body(document: xberg::types::internal::InternalDocument) -> Value {
    let rendered = render_json(&document);
    let parsed: Value = serde_json::from_str(&rendered).expect("render_json must emit valid JSON");
    parsed["body"].clone()
}

#[test]
fn should_emit_a_node_for_every_previously_dropped_element_kind() {
    let body = render_body(document_with_previously_dropped_kinds());

    assert_eq!(
        body,
        json!([
            { "type": "page_break" },
            { "type": "footnote_ref", "number": 1, "id": "fn1" },
            { "type": "footnote_definition", "text": "A footnote body.", "id": "fn1" },
            { "type": "citation", "text": "Smith 2024", "id": "smith2024" },
            { "type": "slide", "number": 3, "title": "Third Slide" },
            { "type": "definition_term", "text": "Rust" },
            { "type": "definition_description", "text": "A systems language" },
            { "type": "admonition", "kind": "warning", "title": "Be careful", "text": "Be careful" },
            { "type": "raw_block", "text": "\\LaTeX{}", "format": "tex" },
            { "type": "metadata_block", "entries": [{ "key": "Author", "value": "Alice" }] },
        ]),
        "every previously-dropped element kind must round-trip into the JSON body"
    );
}

#[test]
fn should_nest_previously_dropped_kinds_inside_the_enclosing_section() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_heading(1, "Chapter 1", None, None);
    builder.push_citation("Smith 2024", "smith2024", None);
    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([{
            "type": "section",
            "heading": "Chapter 1",
            "level": 1,
            "body": [{ "type": "citation", "text": "Smith 2024", "id": "smith2024" }],
        }]),
        "a citation following a heading belongs to that heading's section"
    );
}

#[test]
fn should_omit_title_for_untitled_slide() {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_slide(1, None, Some(7));
    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([{ "type": "slide", "number": 1 }]),
        "a slide with no title must omit the title field rather than emit an empty string"
    );
}

#[test]
fn should_fall_back_to_raw_text_for_unparseable_metadata_block() {
    let mut builder = InternalDocumentBuilder::new("test");
    let index = builder.push_metadata_block(&[("Author".to_string(), "Alice".to_string())], None);
    builder.set_text(index, "no colon here");
    let body = render_body(builder.build());

    assert_eq!(
        body,
        json!([{ "type": "metadata_block", "entries": [], "text": "no colon here" }]),
        "metadata text with no key: value pairs must be preserved verbatim, not dropped"
    );
}
