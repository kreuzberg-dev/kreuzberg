#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
#![cfg(feature = "office")]
//! Regression test for issue #143: when `CitationExtractor` fails to parse a
//! citation file, it silently falls back to returning the raw text as a code
//! block. Under the default build (no `otel` feature), the only diagnostic was
//! a `tracing::warn!` call gated behind `#[cfg(feature = "otel")]`, so callers
//! had no way to learn that parsing failed. The fix adds an unconditional
//! `ProcessingWarning` describing the fallback.

mod helpers;
use helpers::extract_bytes_document;

use xberg::core::config::ExtractionConfig;

const ENDNOTE_XML_MIME: &str = "application/x-endnote+xml";

#[tokio::test]
async fn should_emit_processing_warning_when_citation_parsing_fails() {
    // `biblib`'s RIS parser ignores unparseable lines and, since 0.8, keeps a
    // record that has no title, so RIS can no longer produce a `ParseError`.
    // EndNote XML still can: an unterminated element hits EOF looking for the
    // closing tag.
    let malformed_content = b"<xml><records><record><titles><title>Broken";

    let config = ExtractionConfig::default();
    let extraction = extract_bytes_document(malformed_content, ENDNOTE_XML_MIME, &config)
        .await
        .expect("malformed citation extraction should still succeed with a fallback");

    assert!(
        extraction
            .content
            .contains("<xml><records><record><titles><title>Broken"),
        "expected raw text fallback in body, got: {:?}",
        extraction.content
    );

    let warning = extraction
        .processing_warnings
        .iter()
        .find(|w| w.source == "citation")
        .unwrap_or_else(|| {
            panic!(
                "expected a citation ProcessingWarning, got: {:?}",
                extraction.processing_warnings
            )
        });

    assert_eq!(
        warning.message, "Citation parsing failed; returning raw text as a fallback",
        "unexpected warning message: {:?}",
        warning.message
    );
}
