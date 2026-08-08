#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
#![cfg(feature = "office")]
//! Regression test for issue #122: `CitationExtractor` built a `formatted_content`
//! detail body (journal/year/volume/DOI/PMID/abstract/keywords) for each parsed
//! citation but only ever pushed it into the document on the parse-failure
//! fallback path. On success, `push_citation(title, key, None)` discarded the
//! detail body entirely, so the abstract, DOI, and other bibliographic detail
//! never made it into the extracted document body.

mod helpers;
use helpers::extract_bytes_document;

use xberg::core::config::ExtractionConfig;

const RIS_MIME: &str = "application/x-research-info-systems";

#[tokio::test]
async fn should_include_abstract_and_doi_in_extracted_body_on_successful_parse() {
    let ris_content = br#"TY  - JOUR
TI  - Sample Title
AU  - Smith, John
PY  - 2023
DO  - 10.1234/example.doi
AB  - This is the abstract text for the sample citation.
ER  -"#;

    let config = ExtractionConfig::default();
    let extraction = extract_bytes_document(ris_content, RIS_MIME, &config)
        .await
        .expect("RIS extraction should succeed");

    assert!(
        extraction
            .content
            .contains("Abstract: This is the abstract text for the sample citation."),
        "expected abstract text in body, got: {:?}",
        extraction.content
    );
    assert!(
        extraction.content.contains("DOI: 10.1234/example.doi"),
        "expected DOI in body, got: {:?}",
        extraction.content
    );
}
