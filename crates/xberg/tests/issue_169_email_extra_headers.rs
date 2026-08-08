//! Regression tests for issue #169.
//!
//! `EmailExtractor::build_internal_document` only rendered
//! Subject/From/To/CC/Date into the document body. Bcc, Reply-To,
//! Message-ID, In-Reply-To, References, List-Id, and List-Unsubscribe
//! survived only in `Metadata.additional` and never reached the rendered
//! content. This matters for mail-corpus thread reconstruction. All of these
//! headers must now appear in the rendered content when present, and must be
//! absent (no empty lines) when not present.

#![cfg(feature = "email")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

#[test]
fn should_render_extended_headers_when_present() {
    let config = ExtractionConfig::default();

    let eml_content = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Cc: cc@example.com\r\n\
Bcc: bcc@example.com\r\n\
Reply-To: reply@example.com\r\n\
Subject: Thread reconstruction headers\r\n\
Date: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
Message-ID: <msg1@example.com>\r\n\
In-Reply-To: <parent@example.com>\r\n\
References: <root@example.com> <parent@example.com>\r\n\
List-Id: engineering <eng.example.com>\r\n\
List-Unsubscribe: <mailto:unsub@example.com>\r\n\
\r\n\
Body text.";

    let result =
        extract_bytes_document_blocking(eml_content, "message/rfc822", &config).expect("extraction should succeed");

    assert!(
        result.content.contains("Subject: Thread reconstruction headers"),
        "content was: {}",
        result.content
    );
    assert!(result.content.contains("From: sender@example.com"));
    assert!(result.content.contains("To: recipient@example.com"));
    assert!(result.content.contains("CC: cc@example.com"));
    assert!(
        result.content.contains("BCC: bcc@example.com"),
        "BCC line missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("Reply-To: reply@example.com"),
        "Reply-To line missing from content: {}",
        result.content
    );
    assert!(result.content.contains("Date: Mon, 1 Jan 2024 12:00:00 +0000"));
    assert!(
        result.content.contains("Message-ID:") && result.content.contains("msg1@example.com"),
        "Message-ID line missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("In-Reply-To:") && result.content.contains("parent@example.com"),
        "In-Reply-To line missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("References:") && result.content.contains("root@example.com"),
        "References line missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("List-Id:") && result.content.contains("eng.example.com"),
        "List-Id line missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("List-Unsubscribe:") && result.content.contains("unsub@example.com"),
        "List-Unsubscribe line missing from content: {}",
        result.content
    );
}

#[test]
fn should_omit_extended_header_lines_when_absent() {
    let config = ExtractionConfig::default();

    let eml_content = b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Minimal email\r\n\
Date: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
\r\n\
Body text.";

    let result =
        extract_bytes_document_blocking(eml_content, "message/rfc822", &config).expect("extraction should succeed");

    for absent_prefix in [
        "BCC:",
        "Reply-To:",
        "Message-ID:",
        "In-Reply-To:",
        "References:",
        "List-Id:",
        "List-Unsubscribe:",
    ] {
        assert!(
            !result.content.contains(absent_prefix),
            "unexpected '{}' line in content when header absent: {}",
            absent_prefix,
            result.content
        );
    }
}
