//! Regression test for #134 — XML parsing truncates silently instead of warning.
//!
//! The XML element-tree walk runs with `check_end_names = false`, so a document
//! that is cut off mid-tree never raises a parser error: the reader simply hits
//! EOF with elements still open and the extractor returns `Ok` with a partial
//! tree and no indication that anything was lost.

#![cfg(feature = "xml")]

mod helpers;

use helpers::extract_bytes_document;
use xberg::core::config::ExtractionConfig;

const TRUNCATED_XML: &[u8] = b"<root><item>Hello";
const WELL_FORMED_XML: &[u8] = b"<root><item>Hello</item></root>";

#[tokio::test]
async fn should_warn_when_xml_input_ends_with_unclosed_elements() {
    let result = extract_bytes_document(TRUNCATED_XML, "application/xml", &ExtractionConfig::default())
        .await
        .expect("truncated XML still yields a partial document");

    assert_eq!(
        result.processing_warnings.len(),
        1,
        "expected exactly one truncation warning, got {:?}",
        result.processing_warnings
    );
    let warning = &result.processing_warnings[0];
    assert_eq!(warning.source, "xml");
    assert_eq!(
        warning.message,
        "Input ended with 2 unclosed elements (root, item); the document is truncated \
         and trailing content was not extracted"
    );

    assert!(
        result.content.contains("Hello"),
        "partial content must be preserved alongside the warning: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_not_warn_for_well_formed_xml() {
    let result = extract_bytes_document(WELL_FORMED_XML, "application/xml", &ExtractionConfig::default())
        .await
        .expect("well-formed XML extracts");

    assert!(
        result.processing_warnings.is_empty(),
        "a complete document must not be flagged as truncated: {:?}",
        result.processing_warnings
    );
}
