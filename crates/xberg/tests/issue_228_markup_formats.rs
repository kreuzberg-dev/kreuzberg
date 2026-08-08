//! Regression tests for issue #228: `PlainTextExtractor` advertised seven markup MIME
//! types (`text/asciidoc`, `text/x-asciidoc`, `text/vtt`, `text/troff`, `text/x-mdoc`,
//! `text/x-pod`, `text/x-dokuwiki`) while doing nothing but a BOM strip and a blank-line
//! paragraph split. Every one of them silently produced structureless output that looked
//! like a successful extraction.
//!
//! The resolution splits the seven:
//!
//! - **AsciiDoc** and **WebVTT** are common, well-specified and lose a lot under the
//!   plain-text path, so they now have real extractors. These tests assert the exact
//!   structure those extractors produce.
//! - **troff**, **mdoc**, **POD** and **DokuWiki** are extension-less MIME-only catalogue
//!   entries with no implementation behind them; they were removed from the format
//!   catalogue so callers get an honest `UnsupportedFormat` instead of garbage.

mod helpers;
use helpers::extract_bytes_document;

use xberg::ExtractionConfig;
use xberg::core::mime::validate_mime_type;
use xberg::extractors::ensure_initialized;
use xberg::plugins::registry::get_document_extractor_registry;
use xberg::types::document_structure::NodeContent;

/// Markup MIME types deliberately dropped from the catalogue rather than implemented.
const REMOVED_MIME_TYPES: [&str; 4] = ["text/troff", "text/x-mdoc", "text/x-pod", "text/x-dokuwiki"];

fn structure_config() -> ExtractionConfig {
    ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    }
}

fn nodes(result: &xberg::ExtractedDocument) -> &[xberg::types::document_structure::DocumentNode] {
    result
        .document
        .as_ref()
        .expect("include_document_structure should populate `document`")
        .nodes
        .as_slice()
}

// ---------------------------------------------------------------------------
// AsciiDoc
// ---------------------------------------------------------------------------

const ASCIIDOC_SAMPLE: &str = "= Extraction Guide
Ada Lovelace
:version: 2.1

Intro paragraph for version {version}.

== Getting Started

First section body.

=== Prerequisites

Deep section body.

[source,rust]
----
fn main() {
    println!(\"hi\");
}
----

* alpha
* beta
** beta nested

. first
. second

NOTE: Mind the gap.

.Release matrix
[cols=\">,<\",options=\"header\"]
|===
|Version |Status
|2.1 |stable
|2.2 |beta
|===

See link:https://example.com/docs[the docs] for more.
";

#[tokio::test]
async fn should_extract_asciidoc_document_title_and_section_levels() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    let titles: Vec<&str> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Title { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(titles, vec!["Extraction Guide"], "`= Title` must become a Title node");

    let headings: Vec<(u8, &str)> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Heading { level, text } => Some((*level, text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        headings,
        vec![(2, "Getting Started"), (3, "Prerequisites")],
        "`== ` / `=== ` must become headings at the matching level"
    );

    assert_eq!(
        result.metadata.title.as_deref(),
        Some("Extraction Guide"),
        "the document title must reach document metadata"
    );
    assert_eq!(
        result.metadata.authors.as_deref(),
        Some(["Ada Lovelace".to_string()].as_slice()),
        "the implicit author line must reach document metadata"
    );
}

#[tokio::test]
async fn should_extract_asciidoc_source_block_with_language() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    let code: Vec<(&str, Option<&str>)> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Code { text, language } => Some((text.as_str(), language.as_deref())),
            _ => None,
        })
        .collect();

    assert_eq!(
        code,
        vec![("fn main() {\n    println!(\"hi\");\n}", Some("rust"))],
        "a `[source,rust]` + `----` block must become one Code node with its language"
    );
}

#[tokio::test]
async fn should_extract_asciidoc_lists_with_ordering_and_nesting() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    let items: Vec<&str> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::ListItem { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(items, vec!["alpha", "beta", "beta nested", "first", "second"]);

    let lists: Vec<bool> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::List { ordered } => Some(*ordered),
            _ => None,
        })
        .collect();
    assert_eq!(
        lists,
        vec![false, false, true],
        "one unordered list, its nested unordered child list, then one ordered list"
    );
}

#[tokio::test]
async fn should_extract_asciidoc_table_as_structured_grid() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    assert_eq!(
        result.tables.len(),
        1,
        "the `|===` block must produce exactly one table"
    );
    assert_eq!(
        result.tables[0].cells,
        vec![
            vec!["Version".to_string(), "Status".to_string()],
            vec!["2.1".to_string(), "stable".to_string()],
            vec!["2.2".to_string(), "beta".to_string()],
        ],
        "table cells must be split into a 3x2 grid, not left as pipe-delimited prose"
    );
    assert!(
        !result.content.contains("|==="),
        "table delimiters must not leak into body content: {}",
        result.content
    );
}

#[tokio::test]
async fn should_extract_asciidoc_admonition_with_kind_and_body() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    let admonitions: Vec<(&str, Option<&str>)> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Admonition { kind, title } => Some((kind.as_str(), title.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(admonitions, vec![("note", None)]);
    assert!(
        result.content.contains("Mind the gap."),
        "the admonition body must survive as content: {}",
        result.content
    );
    assert!(
        !result.content.contains("NOTE:"),
        "the raw `NOTE:` label must not remain in body text: {}",
        result.content
    );
}

#[tokio::test]
async fn should_populate_text_metadata_for_asciidoc() {
    let result = extract_bytes_document(
        ASCIIDOC_SAMPLE.as_bytes(),
        "text/asciidoc",
        &ExtractionConfig::default(),
    )
    .await
    .expect("asciidoc extraction should succeed");

    let text_metadata = match result.metadata.format.as_ref().expect("format metadata") {
        xberg::types::FormatMetadata::Text(meta) => meta,
        other => panic!("expected Text format metadata, got {other:?}"),
    };

    assert_eq!(
        text_metadata.headers.as_deref(),
        Some(
            [
                "Extraction Guide".to_string(),
                "Getting Started".to_string(),
                "Prerequisites".to_string(),
            ]
            .as_slice()
        ),
        "TextMetadata.headers was hardcoded None before #228"
    );
    assert_eq!(
        text_metadata.code_blocks.as_deref(),
        Some([("rust".to_string(), "fn main() {\n    println!(\"hi\");\n}".to_string())].as_slice()),
        "TextMetadata.code_blocks was hardcoded None before #228"
    );
    assert_eq!(
        text_metadata.links.as_deref(),
        Some([("the docs".to_string(), "https://example.com/docs".to_string())].as_slice()),
        "TextMetadata.links was hardcoded None before #228"
    );
}

#[tokio::test]
async fn should_substitute_asciidoc_attribute_references() {
    let result = extract_bytes_document(ASCIIDOC_SAMPLE.as_bytes(), "text/asciidoc", &structure_config())
        .await
        .expect("asciidoc extraction should succeed");

    let paragraphs: Vec<&str> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Paragraph { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        paragraphs.contains(&"Intro paragraph for version 2.1."),
        "`{{version}}` must resolve from the `:version:` attribute entry: {paragraphs:?}"
    );
}

#[tokio::test]
async fn should_resolve_asciidoc_x_alias_to_the_same_extractor() {
    ensure_initialized().expect("extractor registration should succeed");
    let registry = get_document_extractor_registry();
    let registry = registry.read();

    let canonical = registry.get("text/asciidoc").expect("text/asciidoc must be routable");
    let alias = registry
        .get("text/x-asciidoc")
        .expect("text/x-asciidoc must be routable");
    assert_eq!(canonical.name(), "asciidoc-extractor");
    assert_eq!(alias.name(), canonical.name());
}

#[tokio::test]
async fn should_degrade_gracefully_on_malformed_asciidoc() {
    // An unterminated source block, an unterminated table, a heading run deeper than
    // AsciiDoc allows, and a dangling attribute line.
    let malformed = b"= Broken\n\n======= not a heading\n\n[source,\n----\nfn oops(\n\n|===\n|only |one\n";

    let result = extract_bytes_document(malformed, "text/asciidoc", &structure_config())
        .await
        .expect("malformed asciidoc must degrade, not fail");

    let asciidoc_warnings: Vec<&str> = result
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == "asciidoc")
        .map(|warning| warning.message.as_ref())
        .collect();
    assert_eq!(
        asciidoc_warnings,
        vec!["unterminated delimited block closed at end of input"],
        "an unterminated block must be reported rather than failing extraction"
    );
    assert!(
        result.content.contains("fn oops("),
        "content inside the unterminated block must be preserved: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// WebVTT
// ---------------------------------------------------------------------------

const VTT_SAMPLE: &str = "WEBVTT - Interview transcript

NOTE This note is not part of the transcript.

STYLE
::cue { color: yellow }

intro
00:00:01.000 --> 00:00:04.000
<v Roger Bingham>We are in New York City

00:00:04.500 --> 00:00:07.250
Second line one
Second line two

00:01:02.003 --> 00:01:05.000
Escaped &amp; <i>styled</i> text
";

#[tokio::test]
async fn should_extract_webvtt_cue_text_without_timestamps_or_identifiers() {
    let result = extract_bytes_document(VTT_SAMPLE.as_bytes(), "text/vtt", &structure_config())
        .await
        .expect("webvtt extraction should succeed");

    let paragraphs: Vec<&str> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Paragraph { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        paragraphs,
        vec![
            "Roger Bingham: We are in New York City",
            "Second line one\nSecond line two",
            "Escaped & styled text",
        ],
        "each cue payload becomes one paragraph, with voice spans, cue tags and character \
         escapes resolved"
    );

    assert!(
        !result.content.contains("-->"),
        "timestamps must never appear in body content: {}",
        result.content
    );
    assert!(
        !result.content.contains("WEBVTT"),
        "the signature line must never appear in body content: {}",
        result.content
    );
    assert!(
        !result.content.contains("intro"),
        "the cue identifier must never appear in body content: {}",
        result.content
    );
}

#[tokio::test]
async fn should_attach_webvtt_timings_as_element_attributes() {
    let result = extract_bytes_document(VTT_SAMPLE.as_bytes(), "text/vtt", &structure_config())
        .await
        .expect("webvtt extraction should succeed");

    let first = nodes(&result)
        .iter()
        .find(|node| matches!(&node.content, NodeContent::Paragraph { .. }))
        .expect("at least one cue paragraph");
    let attributes = first.attributes.as_ref().expect("cue attributes");

    assert_eq!(attributes.get("start").map(String::as_str), Some("00:00:01.000"));
    assert_eq!(attributes.get("end").map(String::as_str), Some("00:00:04.000"));
    assert_eq!(attributes.get("cue_id").map(String::as_str), Some("intro"));
    assert_eq!(attributes.get("speaker").map(String::as_str), Some("Roger Bingham"));
}

#[tokio::test]
async fn should_skip_webvtt_note_and_style_blocks() {
    let result = extract_bytes_document(VTT_SAMPLE.as_bytes(), "text/vtt", &ExtractionConfig::default())
        .await
        .expect("webvtt extraction should succeed");

    assert!(
        !result.content.contains("This note is not part of the transcript."),
        "NOTE blocks are comments and must not become content: {}",
        result.content
    );
    assert!(
        !result.content.contains("::cue"),
        "STYLE blocks must not become content: {}",
        result.content
    );
    assert_eq!(
        result.metadata.title.as_deref(),
        Some("Interview transcript"),
        "trailing text on the WEBVTT signature line is the track title"
    );
}

#[tokio::test]
async fn should_populate_text_metadata_for_webvtt() {
    let result = extract_bytes_document(VTT_SAMPLE.as_bytes(), "text/vtt", &ExtractionConfig::default())
        .await
        .expect("webvtt extraction should succeed");

    let text_metadata = match result.metadata.format.as_ref().expect("format metadata") {
        xberg::types::FormatMetadata::Text(meta) => meta,
        other => panic!("expected Text format metadata, got {other:?}"),
    };

    // Counts are over the extracted cue text only — never over the timing lines.
    // "Roger Bingham: We are in New York City\nSecond line one\nSecond line two\n
    //  Escaped & styled text" — 38 + 15 + 15 + 21 characters plus 3 newlines.
    assert_eq!(
        text_metadata.line_count, 4,
        "3 cues, one of which has two payload lines"
    );
    assert_eq!(text_metadata.word_count, 18);
    assert_eq!(text_metadata.character_count, 92);
    assert_eq!(
        text_metadata.headers, None,
        "WebVTT has no headings to report; the field stays None deliberately"
    );
    assert_eq!(text_metadata.links, None, "WebVTT cue payloads carry no link markup");
    assert_eq!(text_metadata.code_blocks, None, "WebVTT has no code blocks");
}

#[tokio::test]
async fn should_degrade_gracefully_on_malformed_webvtt() {
    // No WEBVTT signature, one cue with an unparsable timing line, one block with no
    // timing line at all, and one valid cue that must still survive.
    let malformed = b"nonsense header\n\nbroken\n00:00:xx --> 99\nDropped payload\n\nstray block with no timing\n\n00:00:02.000 --> 00:00:03.000\nSurviving cue\n";

    let result = extract_bytes_document(malformed, "text/vtt", &structure_config())
        .await
        .expect("malformed webvtt must degrade, not fail");

    let paragraphs: Vec<&str> = nodes(&result)
        .iter()
        .filter_map(|node| match &node.content {
            NodeContent::Paragraph { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        paragraphs,
        vec!["Surviving cue"],
        "the one well-formed cue must still be extracted"
    );

    let vtt_warnings: Vec<&str> = result
        .processing_warnings
        .iter()
        .filter(|warning| warning.source == "vtt")
        .map(|warning| warning.message.as_ref())
        .collect();
    assert_eq!(
        vtt_warnings,
        vec![
            "missing WEBVTT signature line; parsing as WebVTT anyway",
            "block without a timing line skipped",
            "cue with an unparsable timing line skipped",
            "block without a timing line skipped",
        ],
        "every malformed construct must be reported rather than silently dropped"
    );
}

// ---------------------------------------------------------------------------
// Formats removed from the catalogue rather than implemented
// ---------------------------------------------------------------------------

#[test]
fn should_no_longer_validate_removed_markup_mime_types() {
    for mime in REMOVED_MIME_TYPES {
        assert!(
            validate_mime_type(mime).is_err(),
            "{mime} must no longer be advertised as a supported MIME type"
        );
    }
}

#[test]
fn should_have_no_registered_extractor_for_removed_markup_mime_types() {
    ensure_initialized().expect("extractor registration should succeed");
    let registry = get_document_extractor_registry();
    let registry = registry.read();

    for mime in REMOVED_MIME_TYPES {
        assert!(registry.get(mime).is_err(), "{mime} must not resolve to any extractor");
    }
}

#[tokio::test]
async fn should_reject_removed_markup_mime_types_with_unsupported_format() {
    for mime in REMOVED_MIME_TYPES {
        let error = extract_bytes_document(b".TH SOME MANPAGE 1\n.SH NAME\n", mime, &ExtractionConfig::default())
            .await
            .expect_err("removed formats must be rejected, not silently mis-extracted");

        assert!(
            matches!(error, xberg::XbergError::UnsupportedFormat(ref got) if got == mime),
            "expected UnsupportedFormat({mime}), got {error:?}"
        );
    }
}
