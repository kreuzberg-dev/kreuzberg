//! Issue #307 — embedded `.msg` messages must run through the extraction
//! pipeline, not be surfaced as opaque attachment placeholders.
//!
//! An Outlook message can carry an `afEmbeddedMessage` attachment
//! (`PidTagAttachMethod == 5`): a whole Message object stored as a nested CFB
//! *storage* under `__substg1.0_3701000D`, not as a binary stream. Such an
//! attachment has no `__substg1.0_37010102` payload, so the generic
//! attachment-extraction path (`extract_attachment_children`) can never handle
//! it — it is deliberately skipped there because the parser reports it as
//! `message/rfc822` with `data: None`.
//!
//! These tests pin the contract that the embedded message is instead parsed in
//! place while walking the CFB tree and routed through the same
//! `EmailExtractor` document build + pipeline run as a top-level message, so
//! its subject, sender, body and its own attachments are all recovered, and
//! that the recursion is bounded by the shared `SecurityLimits` nesting-depth
//! cap rather than an ad-hoc counter.
//!
//! ## Fixture provenance
//!
//! `test_documents/email/` contains no `.msg` file with an embedded message
//! (`attachment.msg`, `msg_with_attachments_alt.msg` and
//! `msg_with_png_attachment.msg` all carry ordinary binary attachments), and no
//! binary fixtures are added here. Every fixture below is therefore built
//! in-process with the `cfb` crate — the same crate `extraction/email.rs` uses
//! to read real `.msg` files — following the MS-OXMSG 2.4 on-disk layout:
//! 32-byte property-stream header for the top-level message, 24-byte for an
//! embedded message, 8-byte for an attachment, PT_UNICODE string properties in
//! `__substg1.0_<id>001F` streams. The layout is spec-derived and matched to
//! what `extraction/email.rs` actually reads; it has not been diffed against a
//! byte-for-byte Outlook-produced sample.

#![cfg(feature = "email")]

use std::io::{Cursor, Write};

use xberg::core::config::ExtractionConfig;
use xberg::{ArchiveEntry, ExtractedDocument, FormatMetadata, SecurityLimits};

mod helpers;
use helpers::extract_bytes_document_blocking;

const MSG_MIME: &str = "application/vnd.ms-outlook";

const PROP_TYPE_LONG: u16 = 0x0003;
const PROP_TYPE_UNICODE_STRING: u16 = 0x001F;

const PID_SUBJECT: u16 = 0x0037;
const PID_BODY: u16 = 0x1000;
const PID_SENDER_NAME: u16 = 0x0C1A;
const PID_SENDER_EMAIL: u16 = 0x0C1F;
const PID_ATTACH_METHOD: u16 = 0x3705;
const PID_ATTACH_LONG_FILENAME: u16 = 0x3707;
const ATTACH_METHOD_EMBEDDED_MSG: u32 = 5;

/// MS-OXMSG 2.4.2: the top-level message property stream starts with a 32-byte header.
const TOP_LEVEL_PROPERTY_HEADER_LEN: usize = 32;
/// MS-OXMSG 2.4.3: an embedded message's property stream header is 24 bytes.
const EMBEDDED_PROPERTY_HEADER_LEN: usize = 24;
/// MS-OXMSG 2.4.4: an attachment's property stream header is 8 bytes.
const ATTACHMENT_PROPERTY_HEADER_LEN: usize = 8;

type Cfb = cfb::CompoundFile<Cursor<Vec<u8>>>;

/// A 16-byte fixed-length PT_LONG property entry.
fn long_prop_entry(prop_id: u16, value: u32) -> [u8; 16] {
    let mut entry = [0u8; 16];
    entry[0..2].copy_from_slice(&PROP_TYPE_LONG.to_le_bytes());
    entry[2..4].copy_from_slice(&prop_id.to_le_bytes());
    entry[8..12].copy_from_slice(&value.to_le_bytes());
    entry
}

fn write_stream(compound: &mut Cfb, path: &str, bytes: &[u8]) {
    let mut stream = compound.create_stream(path).expect("create stream");
    stream.write_all(bytes).expect("write stream");
}

fn write_unicode_string_prop(compound: &mut Cfb, base: &str, prop_id: u16, value: &str) {
    let path = format!("{base}/__substg1.0_{prop_id:04X}{PROP_TYPE_UNICODE_STRING:04X}");
    let mut bytes: Vec<u8> = value.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]);
    write_stream(compound, &path, &bytes);
}

/// Write a message body (subject / sender / plain-text body) at `base`.
///
/// `base` is `""` for the top-level message, or the storage path of an
/// embedded message.
fn write_message(compound: &mut Cfb, base: &str, subject: &str, body: &str, sender: Option<(&str, &str)>) {
    let header_len = if base.is_empty() {
        TOP_LEVEL_PROPERTY_HEADER_LEN
    } else {
        EMBEDDED_PROPERTY_HEADER_LEN
    };
    write_stream(
        compound,
        &format!("{base}/__properties_version1.0"),
        &vec![0u8; header_len],
    );
    write_unicode_string_prop(compound, base, PID_SUBJECT, subject);
    write_unicode_string_prop(compound, base, PID_BODY, body);
    if let Some((name, address)) = sender {
        write_unicode_string_prop(compound, base, PID_SENDER_NAME, name);
        write_unicode_string_prop(compound, base, PID_SENDER_EMAIL, address);
    }
}

/// Create an attachment storage under `base` and return its path.
fn create_attachment_storage(compound: &mut Cfb, base: &str, index: u32, attach_method: Option<u32>) -> String {
    let path = format!("{base}/__attach_version1.0_#{index:08X}");
    compound.create_storage(&path).expect("create attachment storage");
    let mut properties = vec![0u8; ATTACHMENT_PROPERTY_HEADER_LEN];
    if let Some(method) = attach_method {
        properties.extend_from_slice(&long_prop_entry(PID_ATTACH_METHOD, method));
    }
    write_stream(compound, &format!("{path}/__properties_version1.0"), &properties);
    path
}

/// Attach an `afEmbeddedMessage` to the message at `base`, returning the
/// embedded message's storage path (its own `base` for further nesting).
fn attach_embedded_message(compound: &mut Cfb, base: &str, index: u32) -> String {
    let attach_path = create_attachment_storage(compound, base, index, Some(ATTACH_METHOD_EMBEDDED_MSG));
    let embedded_path = format!("{attach_path}/__substg1.0_3701000D");
    compound
        .create_storage(&embedded_path)
        .expect("create embedded message storage");
    embedded_path
}

/// Attach an ordinary binary file attachment to the message at `base`.
fn attach_file(compound: &mut Cfb, base: &str, index: u32, filename: &str, data: &[u8]) {
    let attach_path = create_attachment_storage(compound, base, index, None);
    write_unicode_string_prop(compound, &attach_path, PID_ATTACH_LONG_FILENAME, filename);
    write_stream(compound, &format!("{attach_path}/__substg1.0_37010102"), data);
}

fn new_compound() -> Cfb {
    cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("create CFB file")
}

fn finish(mut compound: Cfb) -> Vec<u8> {
    compound.flush().expect("flush CFB file");
    compound.into_inner().into_inner()
}

/// Outer message with exactly one embedded message carrying subject, sender and body.
fn msg_with_one_embedded_message() -> Vec<u8> {
    let mut compound = new_compound();
    write_message(
        &mut compound,
        "",
        "Outer message subject",
        "Outer message body text.",
        Some(("Outer Sender", "outer@example.com")),
    );
    let embedded = attach_embedded_message(&mut compound, "", 0);
    write_message(
        &mut compound,
        &embedded,
        "Embedded message subject",
        "Embedded message body text.",
        Some(("Inner Sender", "inner@example.com")),
    );
    finish(compound)
}

fn extract_msg(bytes: &[u8], config: &ExtractionConfig) -> ExtractedDocument {
    extract_bytes_document_blocking(bytes, MSG_MIME, config).expect("MSG extraction should succeed")
}

fn only_child(document: &ExtractedDocument) -> &ArchiveEntry {
    let children = document.children.as_ref().expect("embedded message must yield a child");
    assert_eq!(children.len(), 1, "expected exactly one child, got {}", children.len());
    &children[0]
}

fn has_exact_line(text: &str, expected: &str) -> bool {
    text.lines().any(|line| line == expected)
}

/// The embedded message must come back as a fully extracted child document
/// with its own subject, sender and body — not as an opaque attachment stub.
#[test]
fn should_recover_embedded_message_subject_sender_and_body_as_child_document() {
    let document = extract_msg(&msg_with_one_embedded_message(), &ExtractionConfig::default());
    let child = only_child(&document);

    assert_eq!(child.path, "embedded_message_0.msg");
    assert_eq!(child.mime_type, MSG_MIME);
    assert_eq!(
        child.result.metadata.subject.as_deref(),
        Some("Embedded message subject")
    );

    let Some(FormatMetadata::Email(email)) = child.result.metadata.format.as_ref() else {
        panic!("embedded message child must carry email metadata");
    };
    assert_eq!(email.from_email.as_deref(), Some("inner@example.com"));
    assert_eq!(email.from_name.as_deref(), Some("Inner Sender"));

    let content = &child.result.content;
    assert!(
        has_exact_line(content, "Subject: Embedded message subject"),
        "embedded subject line missing from child content: {content}"
    );
    assert!(
        has_exact_line(content, "From: Inner Sender <inner@example.com>"),
        "embedded sender line missing from child content: {content}"
    );
    assert!(
        has_exact_line(content, "Embedded message body text."),
        "embedded body missing from child content: {content}"
    );
}

/// The embedded message's recovered text must also be inlined into the parent
/// document, the same way an ordinary extracted attachment is — otherwise the
/// body is reachable only by walking `children` and is absent from the
/// parent's rendered content.
#[test]
fn should_inline_embedded_message_text_into_parent_content() {
    let document = extract_msg(&msg_with_one_embedded_message(), &ExtractionConfig::default());

    assert!(
        has_exact_line(&document.content, "Subject: Outer message subject"),
        "outer subject missing: {}",
        document.content
    );
    assert!(
        has_exact_line(&document.content, "Outer message body text."),
        "outer body missing: {}",
        document.content
    );
    assert!(
        has_exact_line(&document.content, "Embedded message body text."),
        "embedded body must be inlined into the parent content: {}",
        document.content
    );
    assert!(
        document.processing_warnings.is_empty(),
        "a well-formed embedded message must not warn: {:?}",
        document.processing_warnings
    );
}

/// An attachment of the embedded message must itself be extracted — both
/// inlined into the embedded message's text and exposed as its child.
#[test]
fn should_recover_attachments_of_the_embedded_message() {
    let mut compound = new_compound();
    write_message(&mut compound, "", "Outer message subject", "Outer body.", None);
    let embedded = attach_embedded_message(&mut compound, "", 0);
    write_message(
        &mut compound,
        &embedded,
        "Embedded message subject",
        "Embedded body.",
        None,
    );
    attach_file(&mut compound, &embedded, 0, "note.txt", b"Inner attachment body");
    let bytes = finish(compound);

    let document = extract_msg(&bytes, &ExtractionConfig::default());
    let child = only_child(&document);

    let grandchildren = child
        .result
        .children
        .as_ref()
        .expect("the embedded message's own attachment must be exposed as a child");
    assert_eq!(grandchildren.len(), 1);
    assert_eq!(grandchildren[0].path, "note.txt");
    // `contains` rather than an exact match: the MIME of a bare text payload is
    // sniffed from its bytes, so the extractor that handles it (plain text vs
    // source code) is not pinned by this test.
    assert!(
        grandchildren[0].result.content.contains("Inner attachment body"),
        "the embedded message's attachment must be extracted: {}",
        grandchildren[0].result.content
    );
    assert!(
        child.result.content.contains("Inner attachment body"),
        "the embedded message's attachment text must be inlined into it: {}",
        child.result.content
    );
}

/// `.msg`-in-`.msg`-in-`.msg` must stop at the shared `SecurityLimits`
/// nesting-depth cap with a warning, instead of recursing for as long as the
/// input nests.
#[test]
fn should_stop_recursing_embedded_messages_at_the_nesting_depth_cap() {
    let mut compound = new_compound();
    write_message(&mut compound, "", "Outer message subject", "Outer body.", None);
    let level_one = attach_embedded_message(&mut compound, "", 0);
    write_message(
        &mut compound,
        &level_one,
        "Level one subject",
        "Level one body text.",
        None,
    );
    let level_two = attach_embedded_message(&mut compound, &level_one, 0);
    write_message(
        &mut compound,
        &level_two,
        "Level two subject",
        "Level two body text.",
        None,
    );
    let level_three = attach_embedded_message(&mut compound, &level_two, 0);
    write_message(
        &mut compound,
        &level_three,
        "Level three subject",
        "Level three body text.",
        None,
    );
    let bytes = finish(compound);

    let config = ExtractionConfig {
        security_limits: Some(SecurityLimits {
            max_nesting_depth: 1,
            ..SecurityLimits::default()
        }),
        ..Default::default()
    };
    let document = extract_msg(&bytes, &config);

    let child = only_child(&document);
    assert_eq!(child.path, "embedded_message_0.msg");
    assert!(
        has_exact_line(&child.result.content, "Level one body text."),
        "the first embedded level is within the cap and must be extracted: {}",
        child.result.content
    );

    for forbidden in ["Level two subject", "Level two body text.", "Level three body text."] {
        assert!(
            !document.content.contains(forbidden),
            "content past the depth cap leaked into the parent: {forbidden}"
        );
        assert!(
            !child.result.content.contains(forbidden),
            "content past the depth cap leaked into the child: {forbidden}"
        );
    }

    let depth_warnings: Vec<&str> = document
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == "msg_embedded_message_extraction")
        .map(|warning| warning.message.as_ref())
        .collect();
    assert_eq!(
        depth_warnings.len(),
        1,
        "the cap must be reported exactly once: {:?}",
        document.processing_warnings
    );
    assert!(
        depth_warnings[0].contains("nesting depth cap reached"),
        "depth-cap warning must explain what was lost: {}",
        depth_warnings[0]
    );
}

/// An attachment that declares `afEmbeddedMessage` but whose message storage is
/// missing must not fail or panic the parent extraction: the parent's own text
/// still comes back, and the loss is reported as a warning rather than swallowed.
#[test]
fn should_extract_parent_and_warn_when_embedded_message_is_unreadable() {
    let mut compound = new_compound();
    write_message(
        &mut compound,
        "",
        "Outer message subject",
        "Outer message body text.",
        None,
    );
    // Declare an embedded message, then omit the `__substg1.0_3701000D` storage
    // it points at — the shape a truncated or corrupted `.msg` produces.
    create_attachment_storage(&mut compound, "", 0, Some(ATTACH_METHOD_EMBEDDED_MSG));
    let bytes = finish(compound);

    let document = extract_msg(&bytes, &ExtractionConfig::default());

    assert!(
        has_exact_line(&document.content, "Outer message body text."),
        "the parent must still extract: {}",
        document.content
    );
    assert!(
        document.children.is_none(),
        "an unreadable embedded message must not produce a child"
    );

    let messages: Vec<&str> = document
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == "email_attachment_extraction")
        .map(|warning| warning.message.as_ref())
        .collect();
    assert_eq!(
        messages,
        vec!["Skipped attachment 'attachment_0': no attachment data available"],
        "the unreadable embedded message must be reported, not swallowed"
    );
}
