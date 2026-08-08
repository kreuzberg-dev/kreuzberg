//! Regression test for defect #53.
//!
//! `From<ExtractedDocument> for InternalDocument` documents that the text content is
//! stashed in `pre_rendered_content` "so the pipeline returns it verbatim instead of
//! trying to re-render from a non-existent element tree". That promise was false:
//! `derive_extraction_result` set `content = render_plain(doc)`, and `render_plain`
//! only walks `doc.elements` — which is empty on that conversion — so the round trip
//! produced an empty string and the plugin's text was lost.

use std::borrow::Cow;

use xberg::ExtractedDocument;
use xberg::core::config::OutputFormat;
use xberg::extraction::derive::derive_extraction_result;
use xberg::types::internal::InternalDocument;

const PLUGIN_TEXT: &str = "Extracted by a foreign-language plugin.";

#[test]
fn should_return_pre_rendered_content_verbatim_when_there_is_no_element_tree() {
    let mut doc = InternalDocument::new("text/plain");
    doc.pre_rendered_content = Some(PLUGIN_TEXT.to_string());

    let result = derive_extraction_result(doc, false, OutputFormat::Plain);

    assert_eq!(result.content, PLUGIN_TEXT);
}

#[test]
fn should_preserve_content_across_an_extracted_document_round_trip() {
    // `ExtractedDocument` has crate-private fields, so it cannot be built with a struct
    // literal from an integration test — build it by assignment instead.
    let mut source = ExtractedDocument::default();
    source.content = PLUGIN_TEXT.to_string();
    source.mime_type = Cow::Borrowed("text/plain");

    let round_tripped = ExtractedDocument::from(InternalDocument::from(source));

    assert_eq!(round_tripped.content, PLUGIN_TEXT);
    assert_eq!(round_tripped.mime_type, "text/plain");
}

/// `formatted_content` honoured `pre_rendered_content` for Markdown and Djot but not for
/// Html or Json, so a plugin that had already produced HTML or JSON had it replaced by a
/// re-render of an empty element tree — an empty document shell.
#[test]
fn should_return_pre_rendered_html_and_json_verbatim_instead_of_rendering_an_empty_tree() {
    for (format, tag) in [(OutputFormat::Html, "html"), (OutputFormat::Json, "json")] {
        let mut doc = InternalDocument::new("text/plain");
        doc.pre_rendered_content = Some(PLUGIN_TEXT.to_string());
        doc.metadata.output_format = Some(tag.to_string());

        let result = derive_extraction_result(doc, false, format.clone());

        assert_eq!(
            result.formatted_content.as_deref(),
            Some(PLUGIN_TEXT),
            "pre-rendered {tag} must be returned verbatim"
        );
    }
}

/// The verbatim path is keyed on the format the plugin actually produced: pre-rendered
/// plain text must not be passed off as HTML.
#[test]
fn should_still_render_html_when_pre_rendered_content_is_a_different_format() {
    let mut doc = InternalDocument::new("text/plain");
    doc.pre_rendered_content = Some(PLUGIN_TEXT.to_string());
    doc.metadata.output_format = Some("markdown".to_string());

    let result = derive_extraction_result(doc, false, OutputFormat::Html);

    assert_ne!(result.formatted_content.as_deref(), Some(PLUGIN_TEXT));
}

#[test]
fn should_not_override_a_real_element_rendering_with_pre_rendered_content() {
    use xberg::types::internal::{ElementKind, InternalElement};

    let mut doc = InternalDocument::new("text/plain");
    doc.push_element(InternalElement::text(ElementKind::Paragraph, "real element text", 0));
    doc.pre_rendered_content = Some("stale pre-rendered text".to_string());

    let result = derive_extraction_result(doc, false, OutputFormat::Plain);

    assert_eq!(result.content, "real element text");
}
