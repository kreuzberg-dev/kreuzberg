//! PDF integration tests that remain specific to the Rust core.
//!
//! Positive-path scenarios live in the shared fixtures that back the
//! multi-language E2E generator. This module keeps only the cases that
//! exercise Rust-specific failure handling or error propagation.

#![cfg(feature = "pdf")]

mod helpers;
use helpers::{extract_bytes_document_blocking, extract_uri_document_blocking};

use helpers::*;
use xberg::PdfConfig;
use xberg::core::config::ExtractionConfig;

/// Corrupted / garbage bytes passed as PDF must return a handled error, not panic.
///
/// This is a regression guard for issue #544: previously, malformed PDFs could
/// trigger a Rust panic via `.unwrap()` / `.expect()` calls in the extraction
/// path, crashing the host process when called through FFI (Python, Node, etc.).
#[test]
fn test_corrupted_pdf_returns_error_not_panic() {
    let config = ExtractionConfig::default();

    let result = extract_bytes_document_blocking(b"not a pdf", "application/pdf", &config);
    assert!(result.is_err(), "Garbage bytes should return Err, not Ok");

    let result = extract_bytes_document_blocking(b"%PDF-1.4\n%%EOF", "application/pdf", &config);
    assert!(result.is_err(), "Truncated PDF should return Err, not Ok");

    let mut noisy = b"%PDF-1.7\n".to_vec();
    noisy.extend(std::iter::repeat_n(0xEFu8, 256));
    let result = extract_bytes_document_blocking(&noisy, "application/pdf", &config);
    assert!(result.is_err(), "Corrupt PDF body should return Err, not Ok");
}

#[test]
fn test_pdf_password_protected_fails_gracefully() {
    if skip_if_missing("pdf/copy_protected.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/copy_protected.pdf");
    let result = extract_uri_document_blocking(&file_path, None, &ExtractionConfig::default());

    match result {
        Ok(extraction_result) => {
            assert_mime_type(&extraction_result, "application/pdf");
            assert!(
                extraction_result.chunks.is_none(),
                "Chunks should be None without chunking config"
            );
            assert!(
                extraction_result.detected_languages.is_none(),
                "Language detection not enabled"
            );
        }
        Err(e) => {
            let error_msg = e.to_string().to_lowercase();
            assert!(
                error_msg.contains("password") || error_msg.contains("protected") || error_msg.contains("encrypted"),
                "Error message should indicate password/protection issue, got: {}",
                e
            );
        }
    }
}

#[test]
fn test_pdf_password_protected_succeeds_with_correct_password() {
    if skip_if_missing("pdf/copy_protected.pdf") {
        return;
    }

    let file_path = get_test_file_path("pdf/copy_protected.pdf");

    let config = ExtractionConfig {
        pdf_options: Some(PdfConfig {
            passwords: Some(vec!["wrong-password".into(), "<correct password>".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&file_path, None, &config);

    match result {
        Ok(extraction_result) => {
            assert_mime_type(&extraction_result, "application/pdf");
            assert!(
                extraction_result.chunks.is_none(),
                "Chunks should be None without chunking config"
            );
            assert!(
                extraction_result.detected_languages.is_none(),
                "Language detection not enabled"
            );
        }
        Err(e) => {
            let error_msg = e.to_string().to_lowercase();
            assert!(
                !error_msg.contains("password") && !error_msg.contains("protected") && !error_msg.contains("encrypted"),
                "Error message should not indicate password/protection issue, got: {e}",
            );
        }
    }
}
