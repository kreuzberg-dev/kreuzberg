//! Regression tests for issue #161.
//!
//! `extraction/email.rs::extract_raw_date_header` and `extract_raw_headers`
//! used to validate the *entire* raw message as UTF-8 before scanning for
//! headers, so a single 8-bit byte anywhere in the body (not just the
//! headers) silently dropped Content-Type, MIME-Version, X-Mailer,
//! User-Agent, List-Id, List-Unsubscribe, and Date. In addition, the
//! `unwrap_or(text.len().min(N))` fallback capped the header-scan boundary at
//! a raw byte offset and then sliced a `&str` at that offset
//! (`&text[..header_end]`), which panics whenever the cap lands in the
//! middle of a multi-byte UTF-8 character.

#![cfg(feature = "email")]

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

fn additional_str<'a>(metadata: &'a xberg::Metadata, key: &str) -> Option<&'a str> {
    metadata
        .additional
        .iter()
        .find(|(k, _)| k.as_ref() == key)
        .and_then(|(_, v)| v.as_str())
}

/// A single non-UTF-8 byte in the body must not suppress header extraction.
#[test]
fn should_extract_headers_when_body_contains_invalid_utf8_byte() {
    let config = ExtractionConfig::default();

    let mut eml_content: Vec<u8> = Vec::new();
    eml_content.extend_from_slice(
        b"From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Subject: Non-UTF8 body\r\n\
Date: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\n\
MIME-Version: 1.0\r\n\
X-Mailer: TestMailer/1.0\r\n\
User-Agent: TestAgent/2.0\r\n\
List-Id: test-list <list.example.com>\r\n\
List-Unsubscribe: <mailto:unsub@example.com>\r\n\
\r\n\
Body with an invalid byte: ",
    );
    // 0xFF is never valid as a standalone UTF-8 byte.
    eml_content.push(0xFF);
    eml_content.extend_from_slice(b" end of body.");

    let result = extract_bytes_document_blocking(&eml_content, "message/rfc822", &config)
        .expect("extraction must succeed even with an invalid UTF-8 byte in the body");

    assert_eq!(
        result.metadata.created_at.as_deref(),
        Some("Mon, 1 Jan 2024 12:00:00 +0000"),
        "Date header must survive a non-UTF-8 body byte"
    );

    assert_eq!(
        additional_str(&result.metadata, "content_type"),
        Some("text/plain; charset=iso-8859-1")
    );
    assert_eq!(additional_str(&result.metadata, "mime_version"), Some("1.0"));
    assert_eq!(additional_str(&result.metadata, "x_mailer"), Some("TestMailer/1.0"));
    assert_eq!(additional_str(&result.metadata, "user_agent"), Some("TestAgent/2.0"));
    assert_eq!(
        additional_str(&result.metadata, "list_id"),
        Some("test-list <list.example.com>")
    );
    assert_eq!(
        additional_str(&result.metadata, "list_unsubscribe"),
        Some("<mailto:unsub@example.com>")
    );
}

/// When the header-scan cap lands in the middle of a multi-byte UTF-8
/// character, extraction must not panic, and must still return sane output.
#[test]
fn should_not_panic_when_header_scan_cap_splits_multibyte_char() {
    let config = ExtractionConfig::default();

    // No `\r\n\r\n` / `\n\n` separator anywhere, so the header-section end
    // falls back to the byte cap (16384 for `extract_raw_headers`, 8192 for
    // `extract_raw_date_header`). Fill with ASCII up to one byte before the
    // smaller cap (8192) so the 3-byte UTF-8 character 'world' snowman
    // (E2 98 83, 3 bytes) straddles that boundary, then keep going past the
    // larger cap (16384) with another straddling multi-byte character so
    // both caps are exercised without ever including a blank line.
    let mut body = String::new();
    body.push_str("From: sender@example.com Subject: no-blank-line-header ");
    // Pad with 'a' so the next multibyte char starts 1 byte before the 8192 cap.
    while body.len() < 8191 {
        body.push('a');
    }
    body.push('☃'); // 3-byte char straddling byte offset 8192.
    while body.len() < 16383 {
        body.push('a');
    }
    body.push('☃'); // 3-byte char straddling byte offset 16384.
    body.push_str("more content after the caps with no header/body separator at all");

    let bytes = body.into_bytes();
    assert!(!bytes.is_empty());

    // Must not panic (a panic here would abort the test binary / poison the
    // test rather than return an `Err`).
    let result = extract_bytes_document_blocking(&bytes, "message/rfc822", &config);

    // Regardless of whether headers were fully recognized in this
    // degenerate no-separator input (mail-parser has nothing to treat as a
    // body without a header/body blank-line separator), the call must
    // complete without panicking and must produce a usable, well-formed
    // result rather than aborting the process.
    let doc = result.expect("must not panic and must return a usable result");
    assert_eq!(doc.mime_type, "message/rfc822");
}
