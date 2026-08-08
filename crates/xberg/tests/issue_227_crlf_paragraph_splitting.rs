#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression test for issue #227: the shared paragraph-splitting helper used by
//! the element-based ("elements") output transform (`crate::extraction::transform`)
//! assumed bare `\n\n` paragraph boundaries. Windows-authored CRLF (`\r\n`) text,
//! classic-Mac lone-CR (`\r`) text, and documents mixing line-ending styles were
//! all mis-split: a `\r\n\r\n` blank line was never recognized as a paragraph
//! boundary, so an entire CRLF document collapsed into one NarrativeText element
//! (or, symmetrically, an internal `\r` inside a mixed-ending document leaked
//! stray carriage returns into element text).
//!
//! These tests exercise the public `extract` API end-to-end with
//! `ResultFormat::ElementBased`, asserting the exact NarrativeText element
//! boundaries and exact text content produced for each line-ending style.

mod helpers;
use helpers::extract_bytes_document;

use xberg::ExtractionConfig;
use xberg::types::ResultFormat;

fn element_based_config() -> ExtractionConfig {
    ExtractionConfig {
        result_format: ResultFormat::ElementBased,
        ..Default::default()
    }
}

fn narrative_texts(elements: &[xberg::types::Element]) -> Vec<&str> {
    elements
        .iter()
        .filter(|e| e.element_type == xberg::types::ElementType::NarrativeText)
        .map(|e| e.text.as_str())
        .collect()
}

#[tokio::test]
async fn should_split_crlf_only_document_into_exact_paragraphs() {
    let content =
        b"First paragraph line one.\r\nFirst paragraph line two.\r\n\r\nSecond paragraph.\r\n\r\nThird paragraph.";
    let config = element_based_config();

    let result = extract_bytes_document(content, "text/plain", &config)
        .await
        .expect("CRLF text extraction should succeed");

    let elements = result.elements.expect("elements output should be populated");
    let paragraphs = narrative_texts(&elements);

    assert_eq!(
        paragraphs,
        vec![
            "First paragraph line one.\nFirst paragraph line two.",
            "Second paragraph.",
            "Third paragraph.",
        ],
        "CRLF blank lines must be recognized as paragraph boundaries, and \\r\\n \
         must normalize to \\n within a paragraph"
    );
}

#[tokio::test]
async fn should_split_lone_cr_document_into_exact_paragraphs() {
    // Classic Mac-style line endings: a lone '\r' with no following '\n'.
    let content = b"Alpha line one.\rAlpha line two.\r\rBeta paragraph.\r\rGamma paragraph.";
    let config = element_based_config();

    let result = extract_bytes_document(content, "text/plain", &config)
        .await
        .expect("lone-CR text extraction should succeed");

    let elements = result.elements.expect("elements output should be populated");
    let paragraphs = narrative_texts(&elements);

    assert_eq!(
        paragraphs,
        vec![
            "Alpha line one.\nAlpha line two.",
            "Beta paragraph.",
            "Gamma paragraph.",
        ],
        "a lone \\r blank line must be recognized as a paragraph boundary"
    );
}

#[tokio::test]
async fn should_split_lf_only_document_into_exact_paragraphs() {
    let content = b"One.\nStill one.\n\nTwo.\n\nThree.";
    let config = element_based_config();

    let result = extract_bytes_document(content, "text/plain", &config)
        .await
        .expect("LF text extraction should succeed");

    let elements = result.elements.expect("elements output should be populated");
    let paragraphs = narrative_texts(&elements);

    assert_eq!(
        paragraphs,
        vec!["One.\nStill one.", "Two.", "Three."],
        "plain LF paragraph splitting must remain unchanged"
    );
}

#[tokio::test]
async fn should_split_mixed_line_ending_document_into_exact_paragraphs() {
    // First blank line is CRLF-style, second is LF-style, third is lone-CR style.
    let content = b"Mix one line a.\r\nMix one line b.\r\n\r\nMix two.\n\nMix three.\r\rMix four.";
    let config = element_based_config();

    let result = extract_bytes_document(content, "text/plain", &config)
        .await
        .expect("mixed-line-ending text extraction should succeed");

    let elements = result.elements.expect("elements output should be populated");
    let paragraphs = narrative_texts(&elements);

    assert_eq!(
        paragraphs,
        vec![
            "Mix one line a.\nMix one line b.",
            "Mix two.",
            "Mix three.",
            "Mix four.",
        ],
        "mixed \\r\\n / \\n / \\r blank lines within one document must all be \
         recognized as paragraph boundaries"
    );
}
