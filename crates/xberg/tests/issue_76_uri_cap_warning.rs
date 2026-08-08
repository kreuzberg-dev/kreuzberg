//! Regression test for defect #76.
//!
//! `InternalDocument::push_uri` caps collection at `MAX_URIS` (100_000) as a DoS
//! guard and used to discard everything past the cap without a trace. The caller
//! then saw a `uris` list of exactly 100_000 entries and no way to tell it apart
//! from a document that genuinely contains 100_000 links.
//!
//! The derivation step now emits a `ProcessingWarning` naming how many URIs were
//! found and how many survived — and only when truncation actually occurred.

use xberg::core::config::OutputFormat;
use xberg::extraction::derive::derive_extraction_result;
use xberg::types::internal::InternalDocument;
use xberg::types::uri::{ExtractedUri, UriKind};

/// Mirror of the private `InternalDocument::MAX_URIS`. If the cap ever moves,
/// this test must be updated deliberately — that is the point.
const MAX_URIS: usize = 100_000;

fn document_with_uris(count: usize) -> InternalDocument {
    let mut doc = InternalDocument::new("html");
    for index in 0..count {
        doc.push_uri(ExtractedUri {
            url: format!("https://example.com/{index}"),
            label: None,
            page: None,
            kind: UriKind::Hyperlink,
        });
    }
    doc
}

#[test]
fn should_warn_with_found_and_kept_counts_when_uri_cap_truncates() {
    let overflow = 123;
    let result = derive_extraction_result(document_with_uris(MAX_URIS + overflow), false, OutputFormat::Plain);

    let uris = result.uris.expect("collected URIs must be present");
    assert_eq!(uris.len(), MAX_URIS, "cap must still bound the collected list");

    assert_eq!(
        result.processing_warnings.len(),
        1,
        "truncation must produce exactly one warning, got {:?}",
        result.processing_warnings
    );
    assert_eq!(result.processing_warnings[0].source, "uris");
    assert_eq!(
        result.processing_warnings[0].message,
        "Collected the first 100000 of 100123 URIs; 123 were dropped at the \
         per-document limit and are missing from the result"
    );
}

#[test]
fn should_not_warn_when_uri_count_is_below_the_cap() {
    let result = derive_extraction_result(document_with_uris(5), false, OutputFormat::Plain);

    assert_eq!(result.uris.expect("collected URIs must be present").len(), 5);
    assert!(
        result.processing_warnings.is_empty(),
        "an untruncated document must stay warning-free, got {:?}",
        result.processing_warnings
    );
}

#[test]
fn should_not_warn_when_uri_count_exactly_reaches_the_cap() {
    let result = derive_extraction_result(document_with_uris(MAX_URIS), false, OutputFormat::Plain);

    assert_eq!(result.uris.expect("collected URIs must be present").len(), MAX_URIS);
    assert!(
        result.processing_warnings.is_empty(),
        "hitting the cap exactly drops nothing and must not warn, got {:?}",
        result.processing_warnings
    );
}
