#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
#![cfg(feature = "office")]
//! Regression test for issue #143: when `BibtexExtractor` fails to parse a
//! `.bib` file, it silently falls back to returning the raw text as a code
//! block. Under the default build (no `otel` feature), the only diagnostic was
//! a `tracing::warn!` call gated behind `#[cfg(feature = "otel")]`, so callers
//! had no way to learn that parsing failed. The fix adds an unconditional
//! `ProcessingWarning` describing the fallback.

mod helpers;
use helpers::extract_bytes_document;

use xberg::core::config::ExtractionConfig;

const BIBTEX_MIME: &str = "application/x-bibtex";

#[tokio::test]
async fn should_emit_processing_warning_when_bibtex_parsing_fails() {
    let malformed_content = b"@article{unterminated,\n  title = {Missing closing brace\n  author = {Someone\n";

    let config = ExtractionConfig::default();
    let extraction = extract_bytes_document(malformed_content, BIBTEX_MIME, &config)
        .await
        .expect("malformed bibtex extraction should still succeed with a fallback");

    let warning = extraction
        .processing_warnings
        .iter()
        .find(|w| w.source == "bibtex")
        .unwrap_or_else(|| {
            panic!(
                "expected a bibtex ProcessingWarning, got: {:?}",
                extraction.processing_warnings
            )
        });

    assert_eq!(
        warning.message, "BibTeX parsing failed; returning raw text as a fallback",
        "unexpected warning message: {:?}",
        warning.message
    );
}
