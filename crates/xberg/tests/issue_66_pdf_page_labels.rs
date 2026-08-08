#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::ExtractionConfig;

/// Issue #66: `test_documents/pdf/with_images.pdf` defines
/// `/PageLabels << /Nums [0 << /S /D /St 4 >> ] >>` in its catalog — its
/// single page is decimal-numbered starting at 4, per ISO 32000-1:2008
/// §12.4.2. Before the fix, `/PageLabels` had zero references anywhere in
/// xberg and this numbering scheme was silently dropped; only the plain
/// 1-based page number was ever available.
#[test]
fn test_page_labels_surfaced_in_metadata_additional() {
    if !helpers::test_documents_available() {
        return;
    }
    let path = helpers::get_test_file_path("pdf/with_images.pdf");
    let bytes = std::fs::read(&path).expect("fixture must be readable");
    let config = ExtractionConfig::default();
    let result = extract_bytes_document_blocking(&bytes, "application/pdf", &config)
        .expect("fixture must extract without error");

    let page_labels = result.metadata.additional.get("page_labels").unwrap_or_else(|| {
        panic!(
            "metadata.additional must carry a 'page_labels' entry; got {:?}",
            result.metadata.additional
        )
    });

    assert_eq!(
        page_labels,
        &serde_json::json!(["4"]),
        "fixture's single page must be labeled '4' per /PageLabels /St 4; got {page_labels:?}"
    );
}

/// A PDF with no `/PageLabels` entry at all must not carry a `page_labels`
/// key — plain 1-based numbering is the default and callers should not have
/// to special-case an absent/trivial label list.
#[test]
fn test_no_page_labels_key_when_pdf_defines_none() {
    if !helpers::test_documents_available() {
        return;
    }
    let path = helpers::get_test_file_path("pdf/code_and_formula.pdf");
    let bytes = std::fs::read(&path).expect("fixture must be readable");
    let config = ExtractionConfig::default();
    let result = extract_bytes_document_blocking(&bytes, "application/pdf", &config)
        .expect("fixture must extract without error");

    assert!(
        result.metadata.additional.get("page_labels").is_none(),
        "code_and_formula.pdf has no /PageLabels; metadata.additional must not carry a page_labels \
         key; got {:?}",
        result.metadata.additional.get("page_labels")
    );
}
