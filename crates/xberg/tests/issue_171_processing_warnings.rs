//! Regression tests for #171 — extractors that recovered from a real content loss
//! must say so in `processing_warnings` instead of returning a document that reads
//! as a complete extraction.
//!
//! Two loss classes are covered here:
//!
//! 1. **Lossy UTF-8 decode** (`html`, `rtf`, `docbook`, `jats`). Each of these
//!    extractors falls back to `String::from_utf8_lossy` on a non-UTF-8 input, which
//!    substitutes U+FFFD for every byte it cannot read. Extraction still returns `Ok`,
//!    so without a warning a mojibake'd document is indistinguishable from a clean one.
//! 2. **Structural total loss** (`opml`). A file that parses as XML but carries no
//!    `<opml>`/`<body>` element yields an empty document, previously with no signal
//!    separating it from an outline file that genuinely has no entries.
//!
//! Every case asserts both directions: the damaged input warns, and an equivalent
//! clean input produces no warning from that extractor.

#![cfg(any(feature = "html", feature = "office", feature = "xml"))]

mod helpers;

use helpers::extract_bytes_document;
use xberg::core::config::ExtractionConfig;

/// Substring identifying the shared lossy-decode message built by
/// `core::diagnostics::push_lossy_decode_warning`.
const LOSSY_DECODE_FRAGMENT: &str =
    "is not valid UTF-8; the undecodable bytes were replaced with the Unicode replacement character";

/// Collect the messages of every warning attributed to `source`.
fn messages_from(document: &xberg::ExtractedDocument, source: &str) -> Vec<String> {
    document
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == source)
        .map(|warning| warning.message.to_string())
        .collect()
}

/// Assert exactly one warning from `source` whose message contains `fragment`.
fn assert_single_warning(document: &xberg::ExtractedDocument, source: &str, fragment: &str) {
    let messages = messages_from(document, source);
    assert_eq!(
        messages.len(),
        1,
        "expected exactly one '{source}' warning, got {:?}",
        document.processing_warnings
    );
    assert!(
        messages[0].contains(fragment),
        "'{source}' warning must contain {fragment:?}, got {:?}",
        messages[0]
    );
}

/// Assert `source` contributed no warning at all.
fn assert_no_warning(document: &xberg::ExtractedDocument, source: &str) {
    let messages = messages_from(document, source);
    assert!(
        messages.is_empty(),
        "a clean document must produce no '{source}' warning, got {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// html
// ---------------------------------------------------------------------------

/// `0xE9` is a bare Latin-1 'é': valid cp1252, invalid UTF-8.
#[cfg(feature = "html")]
const LATIN1_HTML: &[u8] = b"<html><body><p>caf\xE9 cr\xE8me</p></body></html>";
#[cfg(feature = "html")]
const CLEAN_HTML: &[u8] = b"<html><body><p>cafe creme</p></body></html>";

#[cfg(feature = "html")]
#[tokio::test]
async fn should_warn_when_html_source_is_decoded_lossily() {
    let result = extract_bytes_document(LATIN1_HTML, "text/html", &ExtractionConfig::default())
        .await
        .expect("non-UTF-8 HTML still yields a document");

    assert_single_warning(&result, "html", LOSSY_DECODE_FRAGMENT);
    assert!(
        messages_from(&result, "html")[0].contains("HTML source"),
        "the warning must name the HTML source as the lost input"
    );
}

#[cfg(feature = "html")]
#[tokio::test]
async fn should_not_warn_for_valid_utf8_html() {
    let result = extract_bytes_document(CLEAN_HTML, "text/html", &ExtractionConfig::default())
        .await
        .expect("valid UTF-8 HTML extracts");

    assert_no_warning(&result, "html");
}

// ---------------------------------------------------------------------------
// rtf
// ---------------------------------------------------------------------------

#[cfg(feature = "office")]
const LATIN1_RTF: &[u8] = b"{\\rtf1\\ansi\\ansicpg1252 caf\xE9 cr\xE8me}";
#[cfg(feature = "office")]
const CLEAN_RTF: &[u8] = b"{\\rtf1\\ansi\\ansicpg1252 cafe creme}";

#[cfg(feature = "office")]
#[tokio::test]
async fn should_warn_when_rtf_source_is_decoded_lossily() {
    let result = extract_bytes_document(LATIN1_RTF, "application/rtf", &ExtractionConfig::default())
        .await
        .expect("non-UTF-8 RTF still yields a document");

    assert_single_warning(&result, "rtf", LOSSY_DECODE_FRAGMENT);
    assert!(
        messages_from(&result, "rtf")[0].contains("RTF source"),
        "the warning must name the RTF source as the lost input"
    );
}

#[cfg(feature = "office")]
#[tokio::test]
async fn should_not_warn_for_valid_utf8_rtf() {
    let result = extract_bytes_document(CLEAN_RTF, "application/rtf", &ExtractionConfig::default())
        .await
        .expect("valid UTF-8 RTF extracts");

    assert_no_warning(&result, "rtf");
}

// ---------------------------------------------------------------------------
// docbook
// ---------------------------------------------------------------------------

#[cfg(feature = "xml")]
const LATIN1_DOCBOOK: &[u8] = b"<article><para>caf\xE9 cr\xE8me</para></article>";
#[cfg(feature = "xml")]
const CLEAN_DOCBOOK: &[u8] = b"<article><para>cafe creme</para></article>";

#[cfg(feature = "xml")]
#[tokio::test]
async fn should_warn_when_docbook_source_is_decoded_lossily() {
    let result = extract_bytes_document(LATIN1_DOCBOOK, "application/docbook+xml", &ExtractionConfig::default())
        .await
        .expect("non-UTF-8 DocBook still yields a document");

    assert_single_warning(&result, "docbook", LOSSY_DECODE_FRAGMENT);
    assert!(
        messages_from(&result, "docbook")[0].contains("DocBook source"),
        "the warning must name the DocBook source as the lost input"
    );
}

#[cfg(feature = "xml")]
#[tokio::test]
async fn should_not_warn_for_valid_utf8_docbook() {
    let result = extract_bytes_document(CLEAN_DOCBOOK, "application/docbook+xml", &ExtractionConfig::default())
        .await
        .expect("valid UTF-8 DocBook extracts");

    assert_no_warning(&result, "docbook");
}

// ---------------------------------------------------------------------------
// jats
// ---------------------------------------------------------------------------

#[cfg(feature = "xml")]
const LATIN1_JATS: &[u8] = b"<article><body><sec><p>caf\xE9 cr\xE8me</p></sec></body></article>";
#[cfg(feature = "xml")]
const CLEAN_JATS: &[u8] = b"<article><body><sec><p>cafe creme</p></sec></body></article>";

#[cfg(feature = "xml")]
#[tokio::test]
async fn should_warn_when_jats_source_is_decoded_lossily() {
    let result = extract_bytes_document(LATIN1_JATS, "application/x-jats+xml", &ExtractionConfig::default())
        .await
        .expect("non-UTF-8 JATS still yields a document");

    assert_single_warning(&result, "jats", LOSSY_DECODE_FRAGMENT);
    assert!(
        messages_from(&result, "jats")[0].contains("JATS source"),
        "the warning must name the JATS source as the lost input"
    );
}

#[cfg(feature = "xml")]
#[tokio::test]
async fn should_not_warn_for_valid_utf8_jats() {
    let result = extract_bytes_document(CLEAN_JATS, "application/x-jats+xml", &ExtractionConfig::default())
        .await
        .expect("valid UTF-8 JATS extracts");

    assert_no_warning(&result, "jats");
}

// ---------------------------------------------------------------------------
// opml
// ---------------------------------------------------------------------------

/// Well-formed XML, correct `<opml>` root, but no `<body>`: every outline entry a
/// reader would expect is absent from the output.
#[cfg(feature = "office")]
const OPML_WITHOUT_BODY: &[u8] =
    br#"<?xml version="1.0"?><opml version="2.0"><head><title>Feeds</title></head></opml>"#;

/// Well-formed XML that is not OPML at all.
#[cfg(feature = "office")]
const OPML_WITHOUT_ROOT: &[u8] = br#"<?xml version="1.0"?><outlines><entry text="A"/></outlines>"#;

#[cfg(feature = "office")]
const CLEAN_OPML: &[u8] = br#"<?xml version="1.0"?><opml version="2.0"><head><title>Feeds</title></head>
<body><outline text="Example"/></body></opml>"#;

#[cfg(feature = "office")]
#[tokio::test]
async fn should_warn_when_opml_has_no_body_element() {
    let result = extract_bytes_document(OPML_WITHOUT_BODY, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("an OPML file with no body still extracts");

    assert_single_warning(&result, "opml", "has no <body> element");
    assert!(
        messages_from(&result, "opml")[0].contains("the extracted document is empty"),
        "the warning must state that the resulting document is empty"
    );
}

#[cfg(feature = "office")]
#[tokio::test]
async fn should_warn_when_opml_has_no_opml_root_element() {
    let result = extract_bytes_document(OPML_WITHOUT_ROOT, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("a non-OPML XML file routed to the OPML extractor still extracts");

    assert_single_warning(&result, "opml", "has no <opml> root element");
}

#[cfg(feature = "office")]
#[tokio::test]
async fn should_not_warn_for_opml_with_outline_entries() {
    let result = extract_bytes_document(CLEAN_OPML, "text/x-opml", &ExtractionConfig::default())
        .await
        .expect("a complete OPML file extracts");

    assert_no_warning(&result, "opml");
}
