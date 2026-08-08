//! Regression tests for xberg-io/xberg#138: DocBook `<programlisting language="...">`
//! must propagate the `language` attribute into the extracted output.
//!
//! Structured extraction (`InternalDocumentExtractor::extract_content`) must carry the
//! language as the `Code` element's `language` attribute, and the markdown text output
//! (`ExtractedDocument::content`) must carry it as the fenced-code-block info string
//! (e.g. ` ```rust `).

#![cfg(feature = "xml")]

use xberg::core::config::{ExtractInput, ExtractionConfig, OutputFormat};
use xberg::extractors::DocbookExtractor;
use xberg::plugins::{DocumentExtractor, InternalDocumentExtractor};
use xberg::types::internal::ElementKind;

#[tokio::test]
async fn should_capture_programlisting_language_attribute_in_code_element() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Language Test</title>
  <programlisting language="rust">fn main() {}</programlisting>
</article>"#;

    let extractor = DocbookExtractor;
    let config = ExtractionConfig::default();
    let doc = extractor
        .extract_content(xml.as_bytes(), "application/docbook+xml", &config)
        .await
        .expect("extraction should succeed");

    let code_element = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::Code)
        .expect("expected a Code element");

    assert_eq!(code_element.text, "fn main() {}");
    let language = code_element
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("language"))
        .map(|s| s.as_str());
    assert_eq!(language, Some("rust"), "expected language attribute to be 'rust'");
}

#[tokio::test]
async fn should_omit_language_attribute_when_not_specified() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>No Language Test</title>
  <programlisting>plain text block</programlisting>
</article>"#;

    let extractor = DocbookExtractor;
    let config = ExtractionConfig::default();
    let doc = extractor
        .extract_content(xml.as_bytes(), "application/docbook+xml", &config)
        .await
        .expect("extraction should succeed");

    let code_element = doc
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::Code)
        .expect("expected a Code element");

    assert_eq!(code_element.text, "plain text block");
    let has_language = code_element
        .attributes
        .as_ref()
        .is_some_and(|attrs| attrs.contains_key("language"));
    assert!(!has_language, "expected no language attribute when not specified");
}

#[tokio::test]
async fn should_emit_language_as_fence_info_string_in_markdown_output() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Fence Test</title>
  <programlisting language="python">print("hi")</programlisting>
</article>"#;

    let extractor = DocbookExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };
    let input = ExtractInput::from_bytes(xml.as_bytes().to_vec(), "application/docbook+xml".to_string(), None);
    let result = extractor
        .extract(input, &config)
        .await
        .expect("extraction should succeed");

    let formatted = result
        .formatted_content
        .expect("expected formatted_content to be populated for Markdown output_format");
    assert!(
        formatted.contains("```python\nprint(\"hi\")\n```"),
        "expected fenced code block with 'python' info string, got: {formatted}"
    );
}
