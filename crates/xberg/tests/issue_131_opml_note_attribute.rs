#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression tests for issue #131: OPML `<outline>` elements carrying a `_note`
//! attribute (used by outliner tools such as OmniOutliner to attach free-text
//! notes) must have that note content captured in the extraction output.

#![cfg(feature = "office")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// An `<outline>` with a `_note` attribute should have the note text present
/// in the extracted content, associated with its outline item.
#[tokio::test]
async fn should_include_note_text_when_outline_has_underscore_note_attribute() {
    let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Note Test</title>
  </head>
  <body>
    <outline text="Item With Note" _note="This is a free-text note attached to the item." />
  </body>
</opml>"#;

    let result = extract_bytes_document(opml, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("Should extract OPML with _note attribute");

    assert_eq!(
        result.content, "Item With Note (_note: This is a free-text note attached to the item.)",
        "Extracted content should contain the outline text with its note attribute rendered inline, \
         matching the pattern used for other outline attributes (type, description, xmlUrl, htmlUrl)"
    );
}

/// An `<outline>` without a `_note` attribute must not have any spurious note
/// text injected into the output (regression safety for the unaffected path).
#[tokio::test]
async fn should_not_include_note_text_when_outline_has_no_underscore_note_attribute() {
    let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>No Note Test</title>
  </head>
  <body>
    <outline text="Item Without Note" />
  </body>
</opml>"#;

    let result = extract_bytes_document(opml, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("Should extract OPML without _note attribute");

    assert_eq!(
        result.content, "Item Without Note",
        "Extracted content should be exactly the outline text with no note appended"
    );
}

/// A `_note` attribute on a nested outline item should be associated with
/// that specific item, not leak into sibling or parent items.
#[tokio::test]
async fn should_associate_note_with_its_own_nested_outline_item() {
    let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Nested Note Test</title>
  </head>
  <body>
    <outline text="Parent">
      <outline text="Child With Note" _note="Child note content" />
      <outline text="Child Without Note" />
    </outline>
  </body>
</opml>"#;

    let result = extract_bytes_document(opml, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("Should extract nested OPML with mixed _note attributes");

    assert_eq!(
        result.content, "Parent\n  Child With Note (_note: Child note content)\n\n  Child Without Note",
        "Note should render inline with only the outline item that declares the _note attribute"
    );
}

/// An `<outline>` with no `text` attribute but a `_note` attribute must not be
/// dropped entirely: the note content should still surface in the output, and
/// its children must still be processed.
#[tokio::test]
async fn should_surface_note_when_outline_has_no_text_attribute() {
    let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Empty Text Note Test</title>
  </head>
  <body>
    <outline _note="Orphan note with no text">
      <outline text="Child Item" />
    </outline>
  </body>
</opml>"#;

    let result = extract_bytes_document(opml, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("Should extract OPML with empty-text outline carrying a _note");

    assert_eq!(
        result.content, "Orphan note with no text\n\nChild Item",
        "Note on an empty-text outline should surface, and its child must still be extracted"
    );
}
