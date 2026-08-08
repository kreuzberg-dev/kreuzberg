//! Regression test for #235 — HTML detection was unreachable behind the generic XML branch.
//!
//! `detect_mime_type_from_bytes` checked `trimmed.starts_with('<')` → `application/xml`
//! *before* the `<!DOCTYPE html`/`<html` checks, so those two could never fire: every tag
//! matches `<`. Whole documents still routed correctly only because `infer::get` recognises
//! them earlier in the same function, but a bare HTML fragment reached the text fallback and
//! was typed `application/xml`, then handed to the XML extractor.
//!
//! Reordering alone is not sufficient — a fragment opening with `<h2>` matches neither
//! `<!DOCTYPE html` nor `<html`. The fix therefore also recognises HTML by the name of the
//! first element, using a deliberately conservative allowlist that omits names shared with
//! the XML vocabularies this crate extracts (DocBook, JATS, FB2, OPML, RSS). The final group
//! of tests below pins that conservatism: those documents must keep routing to XML.

use xberg::detect_mime_type_from_bytes;

fn detect(bytes: &[u8]) -> String {
    detect_mime_type_from_bytes(bytes).expect("should detect a MIME type")
}

/// Baseline: a well-formed document resolves through `infer::get`, before the text fallback.
#[test]
fn should_detect_a_full_html_document_as_html() {
    let html = b"<!DOCTYPE html><html><head><title>T</title></head><body><p>Hi</p></body></html>";
    assert_eq!(detect(html), "text/html");
}

/// The actual regression: a fragment `infer` does not recognise reaches the text fallback.
#[test]
fn should_detect_a_bare_html_fragment_as_html_not_xml() {
    let fragment = b"<h2>Section title</h2><p>Body text.</p>";
    assert_eq!(
        detect(fragment),
        "text/html",
        "a bare HTML fragment must not be shadowed by the generic XML branch"
    );
}

/// `<!doctype html>` is at least as common as the uppercase spelling.
#[test]
fn should_detect_a_lowercase_doctype_as_html() {
    assert_eq!(detect(b"<!doctype html>\n<div>x</div>"), "text/html");
}

#[test]
fn should_detect_a_div_fragment_as_html() {
    assert_eq!(detect(b"<div class=\"wrapper\"><span>text</span></div>"), "text/html");
}

/// An XML declaration always wins, whatever the first element is.
///
/// `infer::get` recognises the declaration before the text fallback runs and reports
/// `text/xml`, which `is_xml_mime` treats identically to `application/xml`; the point of
/// this test is that a declared XML document never reaches the HTML branch.
#[test]
fn should_keep_an_xml_declaration_as_xml() {
    assert_eq!(detect(b"<?xml version=\"1.0\"?><p>not html</p>"), "text/xml");
}

/// The allowlist must not reroute the XML vocabularies this crate also extracts.
#[test]
fn should_keep_xml_vocabulary_roots_as_xml() {
    for (label, bytes) in [
        ("docbook", &b"<article><title>T</title><para>text</para></article>"[..]),
        ("jats", &b"<article-title>Study</article-title>"[..]),
        ("fictionbook", &b"<FictionBook><description/></FictionBook>"[..]),
        ("opml", &b"<opml version=\"2.0\"><body/></opml>"[..]),
        ("rss", &b"<rss version=\"2.0\"><channel/></rss>"[..]),
        ("generic", &b"<invoice><line-item/></invoice>"[..]),
    ] {
        assert_eq!(detect(bytes), "application/xml", "{label} must stay XML");
    }
}

/// A namespace prefix that collides with an HTML element name must not read as HTML.
#[test]
fn should_keep_a_namespaced_element_as_xml() {
    assert_eq!(
        detect(b"<tr:transaction><tr:id>1</tr:id></tr:transaction>"),
        "application/xml"
    );
}
