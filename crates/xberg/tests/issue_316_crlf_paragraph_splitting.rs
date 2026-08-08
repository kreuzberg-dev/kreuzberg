#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression tests for issue #316: the remaining `.split("\n\n")` paragraph
//! boundaries outside the plain-text extractor fixed in #227.
//!
//! #227 fixed `extractors/text.rs`. The same defect — splitting on a bare `"\n\n"`
//! without first normalizing CRLF / lone-CR — survived at every other site whose
//! input reaches it from a producer that does not guarantee LF-only text:
//!
//! * `extractors/email.rs` — RFC 5322 mandates CRLF and `mail-parser` returns the
//!   decoded body verbatim, so *every* plain-text email collapsed into one paragraph.
//! * `extractors/pst.rs` — `PR_BODY` (0x1000) out of an Outlook message store, same shape.
//! * `pdf/structure/adapters.rs`, `extractors/pdf/{mod,ocr}.rs` — OCR backend output,
//!   which is passed through verbatim; the VLM backend returns model markdown straight
//!   out of an HTTP JSON body and third-party plugin backends return arbitrary strings.
//! * `extractors/djot_format/conversion.rs` — a public API over `ExtractedDocument`,
//!   whose `content` carries no LF-only guarantee (`render_plain` copies element text
//!   verbatim and the `pre_rendered_content` path bypasses rendering entirely).
//!
//! Each test asserts the exact paragraph count *and* the exact paragraph contents,
//! so a regression that merely produces "more than one" paragraph still fails.

mod helpers;

#[cfg(feature = "email")]
mod email_body {
    use super::helpers::extract_bytes_document;
    use xberg::ExtractionConfig;
    use xberg::types::ResultFormat;

    /// Header block emitted by `EmailExtractor::build_internal_document` before the
    /// body paragraphs. It is a `MetadataBlock`, which maps to `NarrativeText` in the
    /// element-based output, so it is part of the exact expected sequence.
    const EXPECTED_HEADER_BLOCK: &str = "Subject: CRLF Body Test\nFrom: sender@example.com\nTo: recipient@example.com";

    fn element_based_config() -> ExtractionConfig {
        ExtractionConfig {
            result_format: ResultFormat::ElementBased,
            use_cache: false,
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
    async fn should_split_email_body_into_exact_paragraphs_when_body_uses_crlf() {
        let eml = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: CRLF Body Test\r\n\
\r\n\
First paragraph line one.\r\n\
First paragraph line two.\r\n\
\r\n\
Second paragraph.\r\n\
\r\n\
Third paragraph.";

        let result = extract_bytes_document(eml, "message/rfc822", &element_based_config())
            .await
            .expect("CRLF email extraction should succeed");

        let elements = result.elements.expect("elements output should be populated");

        assert_eq!(
            narrative_texts(&elements),
            vec![
                EXPECTED_HEADER_BLOCK,
                "First paragraph line one.\nFirst paragraph line two.",
                "Second paragraph.",
                "Third paragraph.",
            ],
            "CRLF blank lines in an email body must be recognized as paragraph boundaries, \
             and \\r\\n must normalize to \\n within a paragraph"
        );
    }

    #[tokio::test]
    async fn should_split_email_body_into_exact_paragraphs_when_body_uses_lone_cr() {
        // Headers stay RFC-conformant CRLF; only the body uses classic-Mac lone CR.
        let eml = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: CRLF Body Test\r\n\
\r\n\
Alpha line one.\rAlpha line two.\r\rBeta paragraph.\r\rGamma paragraph.";

        let result = extract_bytes_document(eml, "message/rfc822", &element_based_config())
            .await
            .expect("lone-CR email extraction should succeed");

        let elements = result.elements.expect("elements output should be populated");

        assert_eq!(
            narrative_texts(&elements),
            vec![
                EXPECTED_HEADER_BLOCK,
                "Alpha line one.\nAlpha line two.",
                "Beta paragraph.",
                "Gamma paragraph.",
            ],
            "a lone \\r blank line in an email body must be recognized as a paragraph boundary"
        );
    }

    #[tokio::test]
    async fn should_split_email_body_into_exact_paragraphs_when_body_uses_lf() {
        let eml = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: CRLF Body Test\r\n\
\r\n\
One.\nStill one.\n\nTwo.\n\nThree.";

        let result = extract_bytes_document(eml, "message/rfc822", &element_based_config())
            .await
            .expect("LF email extraction should succeed");

        let elements = result.elements.expect("elements output should be populated");

        assert_eq!(
            narrative_texts(&elements),
            vec![EXPECTED_HEADER_BLOCK, "One.\nStill one.", "Two.", "Three."],
            "plain LF paragraph splitting in an email body must remain unchanged"
        );
    }
}

/// `extraction_result_to_djot` is a public API taking a caller-supplied
/// `ExtractedDocument`. Nothing in the pipeline guarantees `content` is LF-only:
/// `render_plain` copies each element's text verbatim, and extractors that set
/// `pre_rendered_content` bypass rendering altogether.
mod djot_conversion {
    use xberg::extractors::djot_format::extraction_result_to_djot;
    use xberg::types::ExtractedDocument;

    // `ExtractedDocument` has private fields, so a struct literal with `..Default::default()`
    // is rejected from an integration test (E0451). Build the default and assign instead.
    fn document_with_content(content: &str) -> ExtractedDocument {
        let mut document = ExtractedDocument::default();
        document.content = content.to_string();
        document
    }

    #[test]
    fn should_emit_exact_djot_paragraphs_when_content_uses_crlf() {
        let document = document_with_content("First paragraph.\r\n\r\nSecond paragraph.\r\n\r\nThird paragraph.");

        let djot = extraction_result_to_djot(&document).expect("djot conversion should succeed");

        assert_eq!(
            djot, "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n\n",
            "CRLF blank lines must produce three separate djot paragraphs"
        );
    }

    #[test]
    fn should_emit_exact_djot_paragraphs_when_content_uses_lone_cr() {
        let document = document_with_content("Alpha.\r\rBeta.\r\rGamma.");

        let djot = extraction_result_to_djot(&document).expect("djot conversion should succeed");

        assert_eq!(
            djot, "Alpha.\n\nBeta.\n\nGamma.\n\n",
            "a lone \\r blank line must produce three separate djot paragraphs"
        );
    }

    #[test]
    fn should_emit_exact_djot_paragraphs_when_content_uses_lf() {
        let document = document_with_content("Alpha.\n\nBeta.\n\nGamma.");

        let djot = extraction_result_to_djot(&document).expect("djot conversion should succeed");

        assert_eq!(
            djot, "Alpha.\n\nBeta.\n\nGamma.\n\n",
            "plain LF djot paragraph splitting must remain unchanged"
        );
    }
}

/// OCR page text reaches `pdf::structure::adapters::ocr_text_to_paragraphs` one hop
/// from `OcrBackend::process_image`'s `ExtractedDocument.content`, entirely unfiltered
/// — the two sanitizers on the path (`ocr::processor::validation::strip_control_characters`
/// and `pdf::text::fix_pdf_control_chars`) both explicitly whitelist `\r`.
///
/// A registered plugin backend is the honest stand-in for the real CR producers here
/// (the VLM backend and third-party binding-provided backends), and it makes the
/// assertion deterministic instead of depending on a Tesseract build.
#[cfg(all(feature = "pdf", feature = "ocr"))]
mod pdf_ocr_page_text {
    use super::helpers::extract_uri_document_blocking;
    use async_trait::async_trait;
    use serial_test::serial;
    use std::borrow::Cow;
    use std::sync::Arc;
    use xberg::Result;
    use xberg::core::config::{ExtractionConfig, OcrConfig};
    use xberg::plugins::registry::get_ocr_backend_registry;
    use xberg::plugins::{OcrBackend, OcrBackendType, Plugin};
    use xberg::types::{ExtractedDocument, ResultFormat};

    /// Single-page PDF, so the OCR page-text join in `extract_with_ocr` contributes
    /// no separators of its own and `text` is exactly what the backend returned.
    const SINGLE_PAGE_PDF: &str = "../../test_documents/pdf/fake_memo.pdf";

    struct CrlfOcrBackend {
        /// Unique per test: the shared registry is process-global, so reusing one name
        /// across tests risks an already-registered collision even with `shutdown_all`.
        name: &'static str,
        text: &'static str,
    }

    impl Plugin for CrlfOcrBackend {
        fn name(&self) -> &str {
            self.name
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OcrBackend for CrlfOcrBackend {
        async fn process_image(&self, _image_bytes: &[u8], _config: &OcrConfig) -> Result<ExtractedDocument> {
            let mut document = ExtractedDocument::default();
            document.content = self.text.to_string();
            document.mime_type = Cow::Borrowed("text/plain");
            Ok(document)
        }

        fn supports_language(&self, _language: &str) -> bool {
            true
        }

        fn backend_type(&self) -> OcrBackendType {
            OcrBackendType::Custom
        }
    }

    struct RegistryGuard;

    impl Drop for RegistryGuard {
        fn drop(&mut self) {
            let registry = get_ocr_backend_registry();
            let mut reg = registry.write();
            let _ = reg.shutdown_all();
        }
    }

    fn ocr_paragraphs_for(backend_name: &'static str, backend_text: &'static str) -> Vec<String> {
        let _guard = RegistryGuard;
        let registry = get_ocr_backend_registry();

        {
            let mut reg = registry.write();
            reg.shutdown_all().expect("registry reset should succeed");
            reg.register(Arc::new(CrlfOcrBackend {
                name: backend_name,
                text: backend_text,
            }) as Arc<dyn OcrBackend>)
                .expect("backend registration should succeed");
        }

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: backend_name.to_string(),
                language: vec!["eng".to_string()],
                ..Default::default()
            }),
            force_ocr: true,
            result_format: ResultFormat::ElementBased,
            use_cache: false,
            ..Default::default()
        };

        let result =
            extract_uri_document_blocking(SINGLE_PAGE_PDF, None, &config).expect("force_ocr extraction should succeed");

        result
            .elements
            .expect("elements output should be populated")
            .iter()
            .filter(|element| element.element_type == xberg::types::ElementType::NarrativeText)
            .map(|element| element.text.clone())
            .collect()
    }

    #[serial]
    #[test]
    fn should_split_ocr_page_text_into_exact_paragraphs_when_backend_returns_crlf() {
        assert_eq!(
            ocr_paragraphs_for(
                "issue-316-crlf-ocr",
                "Alpha paragraph.\r\n\r\nBeta paragraph.\r\n\r\nGamma paragraph.",
            ),
            vec![
                "Alpha paragraph.".to_string(),
                "Beta paragraph.".to_string(),
                "Gamma paragraph.".to_string(),
            ],
            "CRLF blank lines in OCR backend output must be recognized as paragraph boundaries"
        );
    }

    #[serial]
    #[test]
    fn should_split_ocr_page_text_into_exact_paragraphs_when_backend_returns_lf() {
        assert_eq!(
            ocr_paragraphs_for(
                "issue-316-lf-ocr",
                "Alpha paragraph.\n\nBeta paragraph.\n\nGamma paragraph.",
            ),
            vec![
                "Alpha paragraph.".to_string(),
                "Beta paragraph.".to_string(),
                "Gamma paragraph.".to_string(),
            ],
            "plain LF OCR paragraph splitting must remain unchanged"
        );
    }
}
