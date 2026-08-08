//! Regression tests for issue #132: djot figure captions, description list
//! term/definition pairs, div classes, and inline math.
//!
//! Djot (<https://djot.net>) support previously mishandled four constructs:
//! - Figure captions: a paragraph consisting solely of an image followed immediately
//!   (no blank line) by text is a figure with caption, not an unrelated paragraph.
//! - Description lists (`: Term\n\n  Definition`) were dropped instead of producing
//!   `DefinitionTerm`/`DefinitionDescription` elements.
//! - Fenced div classes (`::: warning ... :::`) were dropped instead of being
//!   preserved as metadata on the emitted group element.
//! - Inline math (`` $`x^2` ``) fell through to plain, unmarked text.

use xberg::core::config::ExtractionConfig;
use xberg::plugins::InternalDocumentExtractor;
use xberg::types::document_structure::RelationshipKind;
use xberg::types::internal::{ElementKind, RelationshipTarget};

use xberg::extractors::djot_format::DjotExtractor;

#[tokio::test]
async fn should_link_caption_paragraph_to_image_when_no_blank_line_separates_them() {
    let djot = b"![alt text](img.png)\nThis is the caption.";
    let extractor = DjotExtractor;
    let config = ExtractionConfig::default();

    let doc = extractor
        .extract_content(djot, "text/djot", &config)
        .await
        .expect("figure-caption djot should extract");

    let image_idx = doc
        .elements
        .iter()
        .position(|e| matches!(e.kind, ElementKind::Image { .. }))
        .expect("image element present") as u32;
    let caption_idx = doc
        .elements
        .iter()
        .position(|e| matches!(e.kind, ElementKind::Paragraph))
        .expect("caption paragraph element present") as u32;

    assert_eq!(
        doc.elements[caption_idx as usize].text, "This is the caption.",
        "caption paragraph carries the exact trailing text"
    );

    let has_caption_relationship = doc.relationships.iter().any(|r| {
        r.source == caption_idx
            && r.target == RelationshipTarget::Index(image_idx)
            && r.kind == RelationshipKind::Caption
    });
    assert!(
        has_caption_relationship,
        "expected a Caption relationship from the paragraph to the image, got: {:?}",
        doc.relationships
    );
}

#[tokio::test]
async fn should_not_link_paragraph_to_image_when_blank_line_separates_them() {
    let djot = b"![alt text](img.png)\n\nSome unrelated paragraph.";
    let extractor = DjotExtractor;
    let config = ExtractionConfig::default();

    let doc = extractor
        .extract_content(djot, "text/djot", &config)
        .await
        .expect("plain image + separate paragraph djot should extract");

    let has_caption_relationship = doc.relationships.iter().any(|r| r.kind == RelationshipKind::Caption);
    assert!(
        !has_caption_relationship,
        "a blank line must NOT create a figure/caption relationship, got: {:?}",
        doc.relationships
    );

    let paragraph_texts: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Paragraph))
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        paragraph_texts,
        vec!["Some unrelated paragraph."],
        "unrelated paragraph text preserved unchanged"
    );
}

#[tokio::test]
async fn should_parse_description_list_into_term_and_description_elements() {
    let djot = b": Term\n\n  Definition text here.\n";
    let extractor = DjotExtractor;
    let config = ExtractionConfig::default();

    let doc = extractor
        .extract_content(djot, "text/djot", &config)
        .await
        .expect("description list djot should extract");

    let term_texts: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::DefinitionTerm))
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(term_texts, vec!["Term"], "exactly one definition term with exact text");

    let description_texts: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::DefinitionDescription))
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        description_texts,
        vec!["Definition text here."],
        "exactly one definition description with exact text"
    );

    let term_idx = doc
        .elements
        .iter()
        .position(|e| matches!(e.kind, ElementKind::DefinitionTerm))
        .unwrap();
    let description_idx = doc
        .elements
        .iter()
        .position(|e| matches!(e.kind, ElementKind::DefinitionDescription))
        .unwrap();
    assert!(
        term_idx < description_idx,
        "term must precede its description in document order"
    );
}

#[tokio::test]
async fn should_preserve_div_class_as_metadata_on_the_emitted_group() {
    let djot = b"::: warning\nSome content.\n:::\n";
    let extractor = DjotExtractor;
    let config = ExtractionConfig::default();

    let doc = extractor
        .extract_content(djot, "text/djot", &config)
        .await
        .expect("fenced div djot should extract");

    let group_start = doc
        .elements
        .iter()
        .find(|e| matches!(e.kind, ElementKind::GroupStart))
        .expect("GroupStart element present for the fenced div");
    let label = group_start
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("label"))
        .map(String::as_str);
    assert_eq!(
        label,
        Some("warning"),
        "div class preserved as the group's label attribute"
    );

    assert!(
        doc.elements.iter().any(|e| matches!(e.kind, ElementKind::GroupEnd)),
        "GroupEnd element present to close the div"
    );
    assert!(
        doc.elements
            .iter()
            .any(|e| matches!(e.kind, ElementKind::Paragraph) && e.text == "Some content."),
        "div body content still extracted as a paragraph"
    );
}

#[tokio::test]
async fn should_recognize_inline_math_as_a_math_span_not_literal_backtick_text() {
    let djot = "This is $`x^2` inline math.".as_bytes();
    let extractor = DjotExtractor;
    let config = ExtractionConfig::default();

    let doc = extractor
        .extract_content(djot, "text/djot", &config)
        .await
        .expect("inline math djot should extract");

    let paragraph_texts: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Paragraph))
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        paragraph_texts,
        vec!["This is $x^2$ inline math."],
        "inline math rendered as a $-delimited math span, with the backtick verbatim \
         fence stripped and no literal backtick/dollar-backtick sequence remaining"
    );
    assert!(
        !paragraph_texts[0].contains('`'),
        "no literal backtick should remain once inline math is recognized: {:?}",
        paragraph_texts[0]
    );
}
