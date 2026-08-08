//! Regression tests for issue #141: FictionBook (.fb2) footnote reference resolution.
//!
//! FB2 documents reference footnotes/endnotes inline via `<a type="note" href="#id">`
//! (or `xlink:href="#id"`), with the actual note body living in a separate
//! `<body name="notes">` section, keyed by `<section id="...">`. The extractor must
//! match references to their definitions by `id`, and must not panic or silently drop
//! the reference marker when no matching definition exists.

#![cfg(feature = "office")]

use xberg::OutputFormat;
use xberg::core::config::ExtractionConfig;
use xberg::plugins::InternalDocumentExtractor;
use xberg::types::document_structure::RelationshipKind;
use xberg::types::internal::{ElementKind, RelationshipTarget};

/// A footnote reference in the body must resolve, by `id`, to its definition in the
/// `<body name="notes">` section: the `FootnoteRef` element carries the inline marker
/// text and an anchor equal to the note id, the `FootnoteDefinition` element carries the
/// resolved note text under the same anchor, and a `FootnoteReference` relationship links
/// the two so renderers can join them.
#[tokio::test]
async fn should_resolve_footnote_reference_to_matching_definition_by_id() {
    let fb2 = br##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook>
  <body>
    <section>
      <p>This is the main text with a footnote<a type="note" href="#note1">1</a> in it.</p>
    </section>
  </body>
  <body name="notes">
    <section id="note1">
      <p>1</p>
      <p>This is the footnote body text.</p>
    </section>
  </body>
</FictionBook>"##;

    let extractor = xberg::extractors::fictionbook::FictionBookExtractor;
    let doc = extractor
        .extract_content(fb2, "application/x-fictionbook+xml", &ExtractionConfig::default())
        .await
        .expect("extraction should succeed");

    let footnote_ref = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::FootnoteRef)
        .expect("expected a FootnoteRef element");
    assert_eq!(
        footnote_ref.text, "1",
        "footnote marker text should be the inline label"
    );
    assert_eq!(
        footnote_ref.anchor.as_deref(),
        Some("note1"),
        "footnote ref anchor should be the stripped href id"
    );

    let footnote_def = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::FootnoteDefinition)
        .expect("expected a FootnoteDefinition element");
    assert_eq!(
        footnote_def.anchor.as_deref(),
        Some("note1"),
        "footnote definition anchor should be the section id"
    );
    assert_eq!(
        footnote_def.text, "1 This is the footnote body text.",
        "footnote definition text should contain the note body"
    );

    let ref_index = doc
        .elements
        .iter()
        .position(|e| e.kind == ElementKind::FootnoteRef)
        .expect("ref index") as u32;
    let relationship = doc
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::FootnoteReference && r.source == ref_index)
        .expect("expected a FootnoteReference relationship from the ref element");
    match &relationship.target {
        RelationshipTarget::Key(key) => assert_eq!(key, "note1"),
        other => panic!("expected RelationshipTarget::Key(\"note1\"), got: {:?}", other),
    }

    let main_paragraph = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::Paragraph && e.text.contains("main text"))
        .expect("expected the main body paragraph fragment before the footnote ref");
    assert_eq!(main_paragraph.text, "This is the main text with a footnote");
}

/// The resolved footnote text must also make it into rendered markdown output, proving
/// the resolution is observable end-to-end and not just an internal element artifact.
#[tokio::test]
async fn should_render_resolved_footnote_text_in_markdown_output() {
    let fb2 = br##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook>
  <body>
    <section>
      <p>See the reference<a xlink:href="#n42">42</a> here.</p>
    </section>
  </body>
  <body name="notes">
    <section id="n42">
      <p>A resolvable footnote body.</p>
    </section>
  </body>
</FictionBook>"##;

    let extractor = xberg::extractors::fictionbook::FictionBookExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };
    let doc = extractor
        .extract_content(fb2, "application/x-fictionbook+xml", &config)
        .await
        .expect("extraction should succeed");

    let markdown = doc
        .pre_rendered_content
        .clone()
        .expect("markdown output should have been pre-rendered");
    assert!(
        markdown.contains("A resolvable footnote body."),
        "resolved footnote text missing from rendered markdown output: {markdown}"
    );
}

/// A footnote reference with no matching definition (unknown id, or no notes body at
/// all) must degrade gracefully: extraction must not panic or error, and the reference
/// marker text must still be preserved as its own element instead of being silently
/// dropped, even though no `FootnoteDefinition` element exists for it.
#[tokio::test]
async fn should_preserve_unresolved_footnote_reference_marker_without_panicking() {
    let fb2 = br##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook>
  <body>
    <section>
      <p>Dangling reference<a type="note" href="#ghost">7</a> with no definition.</p>
    </section>
  </body>
</FictionBook>"##;

    let extractor = xberg::extractors::fictionbook::FictionBookExtractor;
    let doc = extractor
        .extract_content(fb2, "application/x-fictionbook+xml", &ExtractionConfig::default())
        .await
        .expect("extraction should succeed even with an unresolved footnote reference");

    let footnote_ref = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::FootnoteRef)
        .expect("the dangling footnote reference marker must be preserved as its own element");
    assert_eq!(footnote_ref.text, "7", "marker text must not be dropped");
    assert_eq!(footnote_ref.anchor.as_deref(), Some("ghost"));

    assert!(
        !doc.elements.iter().any(|e| e.kind == ElementKind::FootnoteDefinition),
        "no FootnoteDefinition element should exist when there is no matching notes section"
    );

    let main_paragraph = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::Paragraph && e.text.contains("Dangling reference"))
        .expect("expected the paragraph fragment preceding the dangling reference");
    assert_eq!(main_paragraph.text, "Dangling reference");
}
