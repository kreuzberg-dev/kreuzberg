#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
#![cfg(feature = "office")]
//! Regression test for issue #144: `CitationExtractor::extract_content` returned
//! an empty `InternalDocument` wrapped in `Ok(...)` when invoked with a MIME
//! type outside `supported_mime_types()`, silently discarding the input bytes
//! instead of surfacing an error. This path is unreachable via the normal
//! registry (MIME routing only ever dispatches a supported type to this
//! extractor), but it is latent defensive-code that must fail loudly if ever
//! invoked directly (e.g. by a caller bypassing the registry).

use xberg::XbergError;
use xberg::core::config::ExtractionConfig;
use xberg::extractors::CitationExtractor;
use xberg::plugins::InternalDocumentExtractor;

#[tokio::test]
async fn should_return_error_for_unsupported_mime_type() {
    let extractor = CitationExtractor;
    let content = b"irrelevant content";
    let config = ExtractionConfig::default();

    let unsupported_mime = "application/x-not-a-citation-format";
    assert!(
        !extractor.supported_mime_types().contains(&unsupported_mime),
        "test precondition: mime type must not be in supported_mime_types()"
    );

    let result = extractor.extract_content(content, unsupported_mime, &config).await;

    match result {
        Err(XbergError::UnsupportedFormat(mime)) => {
            assert_eq!(mime, unsupported_mime);
        }
        other => panic!("expected Err(XbergError::UnsupportedFormat(_)), got: {other:?}"),
    }
}
