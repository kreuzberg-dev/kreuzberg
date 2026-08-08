//! Regression tests for issue #153.
//!
//! `extraction/email.rs` used to read MSG attachments only from the
//! `__substg1.0_37010102` binary stream. An embedded message attachment
//! (`PR_ATTACH_METHOD` / `attach_method == 5`, `afEmbeddedMessage`) is stored
//! as a nested CFB *storage* (`__substg1.0_3701000D`), not a stream, so
//! `binary_data` ended up `None` and the attachment was dropped entirely
//! (both as attachment content and from `nested_messages`, which MSG always
//! returned empty).
//!
//! This test builds a minimal, synthetic MSG-shaped CFB file directly with
//! the `cfb` crate (the same crate `extraction/email.rs` uses to parse real
//! `.msg` files) containing one `afEmbeddedMessage` attachment, and drives it
//! through the public extraction API to verify the embedded message's
//! subject/body now survive extraction instead of vanishing silently.
//!
//! Caveat: this is a synthetic, minimal MSG structure (top-level message with
//! no interesting properties of its own, one embedded-message attachment with
//! a subject and plain-text body) built to match the on-disk property-stream
//! layout described by MS-OXMSG 2.4. It has not been validated against a real
//! Outlook-produced `.msg` file containing an embedded message, since no
//! fixture files could be added for this change; the MS-OXMSG header-size
//! conventions (32-byte top-level / 24-byte embedded-message / 8-byte
//! attachment property-stream headers) were applied based on the spec, not
//! verified against a real-world sample.

#![cfg(feature = "email")]

use std::io::{Cursor, Write};

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document_blocking;

const PROP_TYPE_LONG: u16 = 0x0003;
const PROP_TYPE_UNICODE_STRING: u16 = 0x001F;

const PID_ATTACH_METHOD: u16 = 0x3705;
const ATTACH_METHOD_EMBEDDED_MSG: u32 = 5;
const PID_SUBJECT: u16 = 0x0037;
const PID_BODY: u16 = 0x1000;

/// Build a 16-byte fixed-length property entry for a PT_LONG (u32) value.
fn long_prop_entry(prop_id: u16, value: u32) -> [u8; 16] {
    let mut entry = [0u8; 16];
    entry[0..2].copy_from_slice(&PROP_TYPE_LONG.to_le_bytes());
    entry[2..4].copy_from_slice(&prop_id.to_le_bytes());
    // bytes 4..8 are flags, unused by the reader.
    entry[8..12].copy_from_slice(&value.to_le_bytes());
    entry
}

/// Write a UTF-16LE string stream for the given property id at `base`.
fn write_unicode_string_prop<F: std::io::Read + std::io::Write + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    base: &str,
    prop_id: u16,
    value: &str,
) {
    let path = format!("{base}/__substg1.0_{prop_id:04X}{PROP_TYPE_UNICODE_STRING:04X}");
    let mut bytes: Vec<u8> = value.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]); // NUL terminator
    let mut stream = comp.create_stream(&path).expect("create unicode string stream");
    stream.write_all(&bytes).expect("write unicode string stream");
}

/// Build a minimal synthetic `.msg` file containing exactly one attachment,
/// which is an embedded message (`afEmbeddedMessage`) with its own subject
/// and body.
fn build_msg_with_embedded_message(outer_subject: &str, embedded_subject: &str, embedded_body: &str) -> Vec<u8> {
    let mut comp = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("create CFB file");

    // Top-level message property stream: 32-byte header, no properties.
    {
        let mut stream = comp
            .create_stream("/__properties_version1.0")
            .expect("create root properties stream");
        stream.write_all(&[0u8; 32]).expect("write root properties header");
    }
    write_unicode_string_prop(&mut comp, "", PID_SUBJECT, outer_subject);

    // One attachment storage, embedding a full message.
    let attach_path = "/__attach_version1.0_#00000000";
    comp.create_storage(attach_path).expect("create attachment storage");
    {
        let mut buf = vec![0u8; 8]; // attachment property-stream header is always 8 bytes.
        buf.extend_from_slice(&long_prop_entry(PID_ATTACH_METHOD, ATTACH_METHOD_EMBEDDED_MSG));
        let mut stream = comp
            .create_stream(format!("{attach_path}/__properties_version1.0"))
            .expect("create attachment properties stream");
        stream.write_all(&buf).expect("write attachment properties");
    }

    // The embedded message itself, stored as a nested storage (not a stream).
    let embedded_path = format!("{attach_path}/__substg1.0_3701000D");
    comp.create_storage(&embedded_path)
        .expect("create embedded message storage");
    {
        let mut stream = comp
            .create_stream(format!("{embedded_path}/__properties_version1.0"))
            .expect("create embedded properties stream");
        stream.write_all(&[0u8; 24]).expect("write embedded properties header"); // 24-byte header.
    }
    write_unicode_string_prop(&mut comp, &embedded_path, PID_SUBJECT, embedded_subject);
    write_unicode_string_prop(&mut comp, &embedded_path, PID_BODY, embedded_body);

    comp.flush().expect("flush CFB file");
    comp.into_inner().into_inner()
}

/// The embedded message's subject and body must appear in the extracted
/// content instead of the attachment being silently dropped.
#[test]
fn should_extract_embedded_message_subject_and_body() {
    let config = ExtractionConfig::default();

    let msg_bytes = build_msg_with_embedded_message(
        "Outer message subject",
        "Embedded message subject",
        "Embedded message body text.",
    );

    let result = extract_bytes_document_blocking(&msg_bytes, "application/vnd.ms-outlook", &config)
        .expect("extraction of a synthetic MSG with an embedded message should succeed");

    assert!(
        result.content.contains("Embedded message subject"),
        "embedded message subject missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("Embedded message body text."),
        "embedded message body missing from content: {}",
        result.content
    );
    assert!(
        result.content.contains("Outer message subject"),
        "outer message subject missing from content: {}",
        result.content
    );
}

/// A regression guard: an attachment that merely *declares*
/// `attach_method == afEmbeddedMessage` but has no backing storage at the
/// expected path must not panic, and should fall back to being treated as a
/// normal (dataless) attachment rather than crashing the extractor.
#[test]
fn should_not_panic_when_embedded_message_storage_is_missing() {
    let config = ExtractionConfig::default();

    let mut comp = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("create CFB file");
    {
        let mut stream = comp
            .create_stream("/__properties_version1.0")
            .expect("create root properties stream");
        stream.write_all(&[0u8; 32]).expect("write root properties header");
    }
    write_unicode_string_prop(&mut comp, "", PID_SUBJECT, "Lying attach_method");

    let attach_path = "/__attach_version1.0_#00000000";
    comp.create_storage(attach_path).expect("create attachment storage");
    {
        let mut buf = vec![0u8; 8];
        buf.extend_from_slice(&long_prop_entry(PID_ATTACH_METHOD, ATTACH_METHOD_EMBEDDED_MSG));
        let mut stream = comp
            .create_stream(format!("{attach_path}/__properties_version1.0"))
            .expect("create attachment properties stream");
        stream.write_all(&buf).expect("write attachment properties");
    }
    // Deliberately do NOT create `__substg1.0_3701000D` under the attachment.

    comp.flush().expect("flush CFB file");
    let msg_bytes = comp.into_inner().into_inner();

    let result = extract_bytes_document_blocking(&msg_bytes, "application/vnd.ms-outlook", &config);
    let doc = result.expect("must not panic and must return a usable result");
    assert!(doc.content.contains("Lying attach_method"));
}
