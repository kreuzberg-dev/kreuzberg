//! Regression test for #133 — FictionBook parsing truncates silently instead of warning.
//!
//! The FB2 body walk exited its event loop on the first malformed XML event
//! (`Err(_) => break`) and returned `Ok` with whatever sections it had already
//! collected, so a book truncated at chapter two was indistinguishable from a
//! complete one.

#![cfg(feature = "office")]

mod helpers;

use helpers::extract_bytes_document;
use xberg::core::config::ExtractionConfig;

/// An FB2 book whose body is cut short by an unterminated XML comment: the
/// comment is never closed, so the reader fails at EOF and everything after the
/// first paragraph is unreachable.
const TRUNCATED_FB2: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook>
  <description><title-info><book-title>Broken Book</book-title></title-info></description>
  <body>
    <section>
      <p>First paragraph survives.</p>
      <!-- the remainder of this book is unreachable
      <p>Second paragraph is lost.</p>
    </section>
  </body>
</FictionBook>"#;

const WELL_FORMED_FB2: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook>
  <description><title-info><book-title>Whole Book</book-title></title-info></description>
  <body>
    <section>
      <p>First paragraph survives.</p>
      <p>Second paragraph survives too.</p>
    </section>
  </body>
</FictionBook>"#;

#[tokio::test]
async fn should_warn_when_fictionbook_body_parse_stops_early() {
    let result = extract_bytes_document(
        TRUNCATED_FB2,
        "application/x-fictionbook+xml",
        &ExtractionConfig::default(),
    )
    .await
    .expect("a truncated FB2 still yields the sections read so far");

    assert_eq!(
        result.processing_warnings.len(),
        1,
        "expected exactly one truncation warning, got {:?}",
        result.processing_warnings
    );
    let warning = &result.processing_warnings[0];
    assert_eq!(warning.source, "fictionbook");
    assert_eq!(
        warning.message,
        "Parsing of the FictionBook body stopped early at a malformed XML event; \
         the remaining content was not extracted \
         (cause: syntax error: comment not closed: `-->` not found before end of input)"
    );

    assert!(
        result.content.contains("First paragraph survives."),
        "partial results must be preserved: {:?}",
        result.content
    );
    assert!(
        !result.content.contains("Second paragraph is lost."),
        "content after the malformed event cannot have been read: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_not_warn_for_well_formed_fictionbook() {
    let result = extract_bytes_document(
        WELL_FORMED_FB2,
        "application/x-fictionbook+xml",
        &ExtractionConfig::default(),
    )
    .await
    .expect("well-formed FB2 extracts");

    assert!(
        result.processing_warnings.is_empty(),
        "a complete book must not be flagged as truncated: {:?}",
        result.processing_warnings
    );
    assert!(result.content.contains("Second paragraph survives too."));
}
