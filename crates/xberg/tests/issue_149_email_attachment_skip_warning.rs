//! Regression tests for issue #149.
//!
//! `extract_attachment_children` in `extractors/email.rs` used to silently
//! `continue` (drop) attachments whose MIME type could not be sniffed, or
//! whose `data` was `None`/empty, with no warning surfaced anywhere. The
//! attachment still appeared in the rendered attachment list, so the output
//! looked complete even though the attachment's content was dropped. Both
//! skip paths must now push a `ProcessingWarning` naming the attachment.

#![cfg(feature = "email")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

/// An attachment with a body that cannot be MIME-sniffed (no extension, no
/// recognizable magic bytes, generic declared content-type) must produce a
/// `ProcessingWarning` mentioning its filename, not be silently dropped.
#[test]
fn should_warn_when_attachment_mime_type_cannot_be_sniffed() {
    let config = ExtractionConfig::default();

    let eml_content = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Unsniffable attachment\r\n\
Content-Type: multipart/mixed; boundary=\"boundary123\"\r\n\
\r\n\
--boundary123\r\n\
Content-Type: text/plain\r\n\
\r\n\
Email body text.\r\n\
--boundary123\r\n\
Content-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=\"mystery.dat\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
gYKDhIWGh4iJig==\r\n\
--boundary123--\r\n";

    let result = extract_bytes_document_blocking(eml_content, "message/rfc822", &config)
        .expect("extraction should succeed even when an attachment can't be sniffed");

    assert!(
        result
            .processing_warnings
            .iter()
            .any(|w| w.message.contains("mystery.dat")),
        "expected a processing warning naming the unsniffable attachment, got: {:?}",
        result.processing_warnings
    );
}

/// An attachment whose `data` is unavailable (e.g. an embedded
/// `message/rfc822` sibling case aside — here modeled via an empty body part)
/// must also produce a warning rather than vanishing without a trace.
#[test]
fn should_warn_when_attachment_has_no_data() {
    let config = ExtractionConfig::default();

    let eml_content = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Empty attachment\r\n\
Content-Type: multipart/mixed; boundary=\"boundary123\"\r\n\
\r\n\
--boundary123\r\n\
Content-Type: text/plain\r\n\
\r\n\
Email body text.\r\n\
--boundary123\r\n\
Content-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=\"empty.bin\"\r\n\
\r\n\
--boundary123--\r\n";

    let result = extract_bytes_document_blocking(eml_content, "message/rfc822", &config)
        .expect("extraction should succeed even when an attachment has no data");

    assert!(
        result
            .processing_warnings
            .iter()
            .any(|w| w.message.contains("empty.bin")),
        "expected a processing warning naming the empty-data attachment, got: {:?}",
        result.processing_warnings
    );
}
