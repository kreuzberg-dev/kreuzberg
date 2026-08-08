//! Regression tests for xberg-io/xberg#125: DocBook `<simpara>`, `<variablelist>`,
//! and cross-reference (`<xref>`/`<link>`) handling.
//!
//! `simpara` is DocBook's "simple paragraph" element (no nested block content
//! allowed) and must extract exactly like `<para>`. `variablelist` is a
//! definition-list-like structure (`varlistentry` -> `term` + `listitem`) and
//! must preserve term/definition pairs. `xref linkend="..."` and empty
//! `link linkend="..."` must resolve to readable text (via `xreflabel`, the
//! target's title, or a `[linkend]` fallback) instead of being dropped.

#![cfg(feature = "xml")]

use xberg::core::config::ExtractionConfig;
use xberg::extractors::DocbookExtractor;
use xberg::plugins::InternalDocumentExtractor;
use xberg::types::internal::{ElementKind, InternalDocument};

async fn extract(xml: &str) -> InternalDocument {
    let extractor = DocbookExtractor;
    let config = ExtractionConfig::default();
    extractor
        .extract_content(xml.as_bytes(), "application/docbook+xml", &config)
        .await
        .expect("extraction should succeed")
}

#[tokio::test]
async fn should_extract_simpara_as_paragraph_text() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Simpara Test</title>
  <simpara>This is a simple paragraph.</simpara>
</article>"#;

    let doc = extract(xml).await;
    let has_paragraph = doc
        .elements
        .iter()
        .any(|e| e.kind == ElementKind::Paragraph && e.text == "This is a simple paragraph.");
    assert!(
        has_paragraph,
        "expected a Paragraph element with exact text 'This is a simple paragraph.', got elements: {:?}",
        doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn should_extract_variablelist_as_definition_term_and_description_pairs() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Variablelist Test</title>
  <variablelist>
    <varlistentry>
      <term>Rust</term>
      <listitem><para>A systems programming language.</para></listitem>
    </varlistentry>
    <varlistentry>
      <term>Xberg</term>
      <listitem><para>A document intelligence library.</para></listitem>
    </varlistentry>
  </variablelist>
</article>"#;

    let doc = extract(xml).await;

    let terms: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| e.kind == ElementKind::DefinitionTerm)
        .map(|e| e.text.as_str())
        .collect();
    let descriptions: Vec<&str> = doc
        .elements
        .iter()
        .filter(|e| e.kind == ElementKind::DefinitionDescription)
        .map(|e| e.text.as_str())
        .collect();

    assert_eq!(terms, vec!["Rust", "Xberg"], "unexpected terms: {terms:?}");
    assert_eq!(
        descriptions,
        vec!["A systems programming language.", "A document intelligence library."],
        "unexpected descriptions: {descriptions:?}"
    );
}

#[tokio::test]
async fn should_resolve_xref_to_target_title() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Xref Test</title>
  <section id="setup">
    <title>Setup Instructions</title>
    <para>Content of the setup section.</para>
  </section>
  <para>See <xref linkend="setup"/> for details.</para>
</article>"#;

    let doc = extract(xml).await;
    let has_resolved_xref = doc
        .elements
        .iter()
        .any(|e| e.kind == ElementKind::Paragraph && e.text == "See Setup Instructions for details.");
    assert!(
        has_resolved_xref,
        "expected xref resolved to target title 'Setup Instructions', got elements: {:?}",
        doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn should_resolve_xref_using_xreflabel_attribute() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Xreflabel Test</title>
  <section id="setup">
    <title>Setup Instructions</title>
    <para>Content.</para>
  </section>
  <para>See <xref linkend="setup" xreflabel="the setup guide"/> for details.</para>
</article>"#;

    let doc = extract(xml).await;
    let has_resolved_xref = doc
        .elements
        .iter()
        .any(|e| e.kind == ElementKind::Paragraph && e.text == "See the setup guide for details.");
    assert!(
        has_resolved_xref,
        "expected xref resolved via xreflabel to 'the setup guide', got elements: {:?}",
        doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn should_fall_back_to_bracketed_linkend_when_target_unresolved() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Unresolved Xref Test</title>
  <para>See <xref linkend="nonexistent"/> for details.</para>
</article>"#;

    let doc = extract(xml).await;
    let has_fallback = doc
        .elements
        .iter()
        .any(|e| e.kind == ElementKind::Paragraph && e.text == "See [nonexistent] for details.");
    assert!(
        has_fallback,
        "expected fallback '[nonexistent]' text, got elements: {:?}",
        doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
    );
}
