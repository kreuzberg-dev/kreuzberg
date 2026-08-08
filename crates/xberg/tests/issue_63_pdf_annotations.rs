#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression test for issue #63: PDF annotation subtype widening + rendering.
//!
//! Before this fix, `crates/xberg/src/pdf/oxide/annotations.rs` collapsed
//! Ink/Square/Circle/Polygon/Squiggly/Caret/FileAttachment/Sound/Movie annotations
//! all down to `PdfAnnotationType::Other`, and `PdfAnnotation` carried no author,
//! modification date, colour, subject, or `/QuadPoints`-derived text — so a
//! Highlight annotation never captured the text it actually highlighted. No
//! renderer emitted annotations into the document's rendered output at all.
//!
//! This test builds a minimal single-page PDF with one annotation object per
//! previously-collapsed subtype plus a Highlight whose `/QuadPoints` cover the
//! page's only line of text, then verifies:
//!   (a) each previously-collapsed subtype now round-trips as its own variant,
//!   (b) the Highlight's `marked_text` captures the highlighted text via QuadPoints,
//!   (c) the rendered document (Markdown output) contains the annotation appendix.

#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::PdfAnnotationType;
use xberg::core::config::{ExtractionConfig, OutputFormat, PdfConfig};

/// Build a single-page PDF containing:
/// - one line of body text, "Hello World", at (72, 700) in 24pt Helvetica
/// - a Highlight annotation whose `/QuadPoints` cover that line, with
///   `/T` (author), `/Subj`, `/M`, `/C` (yellow), and `/Contents` all set
/// - one minimal annotation object for each of the nine subtypes that were
///   previously collapsed into `PdfAnnotationType::Other`: Ink, Square,
///   Circle, Polygon, Squiggly, Caret, FileAttachment, Sound, Movie
fn make_pdf_with_annotations() -> Vec<u8> {
    let content_stream = b"BT /F1 24 Tf 72 700 Td (Hello World) Tj ET";

    let mut pdf: Vec<u8> = Vec::new();

    macro_rules! push_bytes {
        ($s:expr) => {
            pdf.extend_from_slice($s)
        };
    }
    macro_rules! push_str {
        ($s:expr) => {
            pdf.extend_from_slice($s.as_bytes())
        };
    }

    push_bytes!(b"%PDF-1.4\n");

    let off1 = pdf.len();
    push_bytes!(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    let off2 = pdf.len();
    push_bytes!(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    let off4 = pdf.len();
    push_str!(format!("4 0 obj\n<< /Length {} >>\nstream\n", content_stream.len()));
    push_bytes!(content_stream);
    push_bytes!(b"\nendstream\nendobj\n");

    let off3 = pdf.len();
    push_bytes!(
        b"3 0 obj\n\
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 800]\n\
            /Resources << /Font << /F1 5 0 R >> >>\n\
            /Contents 4 0 R\n\
            /Annots [6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R]\n\
            >>\n\
         endobj\n"
    );

    let off5 = pdf.len();
    push_bytes!(
        b"5 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica\n\
            /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );

    // Highlight covering the whole "Hello World" line generously, with the
    // full set of newly-added metadata fields populated.
    let off6 = pdf.len();
    push_bytes!(
        b"6 0 obj\n\
         << /Type /Annot /Subtype /Highlight /Rect [60 690 340 725]\n\
            /QuadPoints [60 725 340 725 60 690 340 690]\n\
            /T (Alice Reviewer) /Subj (Reviewer note) /M (D:20240115120000Z)\n\
            /C [1 0.9 0] /Contents (see highlighted text) >>\n\
         endobj\n"
    );

    let off7 = pdf.len();
    push_bytes!(b"7 0 obj\n<< /Type /Annot /Subtype /Ink /Rect [10 10 50 50] >>\nendobj\n");

    let off8 = pdf.len();
    push_bytes!(b"8 0 obj\n<< /Type /Annot /Subtype /Square /Rect [10 60 50 100] >>\nendobj\n");

    let off9 = pdf.len();
    push_bytes!(b"9 0 obj\n<< /Type /Annot /Subtype /Circle /Rect [10 110 50 150] >>\nendobj\n");

    let off10 = pdf.len();
    push_bytes!(b"10 0 obj\n<< /Type /Annot /Subtype /Polygon /Rect [10 160 50 200] >>\nendobj\n");

    let off11 = pdf.len();
    push_bytes!(b"11 0 obj\n<< /Type /Annot /Subtype /Squiggly /Rect [10 210 50 250] >>\nendobj\n");

    let off12 = pdf.len();
    push_bytes!(b"12 0 obj\n<< /Type /Annot /Subtype /Caret /Rect [10 260 50 300] >>\nendobj\n");

    let off13 = pdf.len();
    push_bytes!(b"13 0 obj\n<< /Type /Annot /Subtype /FileAttachment /Rect [10 310 50 350] >>\nendobj\n");

    let off14 = pdf.len();
    push_bytes!(b"14 0 obj\n<< /Type /Annot /Subtype /Sound /Rect [10 360 50 400] >>\nendobj\n");

    let off15 = pdf.len();
    push_bytes!(b"15 0 obj\n<< /Type /Annot /Subtype /Movie /Rect [10 410 50 450] >>\nendobj\n");

    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 16\n\
         0000000000 65535 f \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n",
        off1, off2, off3, off4, off5, off6, off7, off8, off9, off10, off11, off12, off13, off14, off15
    ));
    push_str!(format!(
        "trailer\n<< /Size 16 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    pdf
}

fn extraction_config(output_format: OutputFormat) -> ExtractionConfig {
    ExtractionConfig {
        output_format,
        pdf_options: Some(PdfConfig {
            extract_annotations: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// (a) Every previously-collapsed subtype now round-trips as its own distinct
/// `PdfAnnotationType` variant instead of being flattened into `Other`.
#[test]
fn test_previously_collapsed_subtypes_round_trip_as_distinct_variants() {
    let pdf = make_pdf_with_annotations();
    let config = extraction_config(OutputFormat::Plain);
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("PDF with annotations must extract successfully");

    let annotations = result
        .annotations
        .as_ref()
        .expect("annotation extraction was enabled, so annotations must be Some");

    let expected: &[(&str, PdfAnnotationType)] = &[
        ("Ink", PdfAnnotationType::Ink),
        ("Square", PdfAnnotationType::Square),
        ("Circle", PdfAnnotationType::Circle),
        ("Polygon", PdfAnnotationType::Polygon),
        ("Squiggly", PdfAnnotationType::Squiggly),
        ("Caret", PdfAnnotationType::Caret),
        ("FileAttachment", PdfAnnotationType::FileAttachment),
        ("Sound", PdfAnnotationType::Sound),
        ("Movie", PdfAnnotationType::Movie),
    ];

    for (label, expected_type) in expected {
        let count = annotations
            .iter()
            .filter(|a| a.annotation_type == *expected_type)
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one {label} annotation mapped to {expected_type:?}; got annotations: {annotations:?}"
        );
    }

    let other_count = annotations
        .iter()
        .filter(|a| a.annotation_type == PdfAnnotationType::Other)
        .count();
    assert_eq!(
        other_count, 0,
        "no annotation in this fixture should map to Other; got: {annotations:?}"
    );

    assert_eq!(
        annotations.len(),
        10,
        "expected 9 shape/media annotations plus 1 highlight"
    );
}

/// (b) A Highlight annotation's `/QuadPoints` are used to recover the actual
/// highlighted page text into `marked_text`, and the metadata fields (author,
/// subject, modification date, colour) are all populated from the PDF.
#[test]
fn test_highlight_captures_marked_text_via_quad_points_and_metadata() {
    let pdf = make_pdf_with_annotations();
    let config = extraction_config(OutputFormat::Plain);
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("PDF with annotations must extract successfully");

    let annotations = result.annotations.as_ref().expect("annotations must be Some");
    let highlight = annotations
        .iter()
        .find(|a| a.annotation_type == PdfAnnotationType::Highlight)
        .expect("the fixture has exactly one Highlight annotation");

    assert_eq!(highlight.page_number, 1);
    assert_eq!(highlight.author.as_deref(), Some("Alice Reviewer"));
    assert_eq!(highlight.subject.as_deref(), Some("Reviewer note"));
    assert_eq!(highlight.modified.as_deref(), Some("D:20240115120000Z"));
    assert_eq!(highlight.content.as_deref(), Some("see highlighted text"));
    assert_eq!(highlight.color.as_deref(), Some("#ffe600"));

    let quad_points = highlight
        .quad_points
        .as_ref()
        .expect("Highlight must carry QuadPoints-derived bounding boxes");
    assert_eq!(quad_points.len(), 1, "the fixture's /QuadPoints has a single quad");

    let marked_text = highlight
        .marked_text
        .as_deref()
        .expect("marked_text must be recovered from the page text under /QuadPoints");
    assert_eq!(
        marked_text, "Hello World",
        "marked_text must equal the actual highlighted page text, not the /Contents comment"
    );
}

/// (c) The rendered document (Markdown output) contains an annotations
/// appendix describing every extracted annotation.
#[test]
fn test_rendered_markdown_contains_annotation_output() {
    let pdf = make_pdf_with_annotations();
    let config = extraction_config(OutputFormat::Markdown);
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("PDF with annotations must extract successfully");

    assert!(
        result.content.contains("## Annotations"),
        "rendered markdown must contain the annotations appendix heading; got: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains("**Highlight** (page 1) by Alice Reviewer: Hello World"),
        "rendered markdown must describe the Highlight with its marked text; got: {}",
        result.content
    );
    assert!(
        result.content.contains("**Ink** (page 1)"),
        "rendered markdown must describe the Ink annotation by its own label, not \
         collapse it into a generic entry; got: {}",
        result.content
    );
}

/// (c, plain-text variant) The default `Plain` output format also carries the
/// annotations appendix, since `render_plain` is the always-computed base
/// `content` field regardless of `output_format`.
#[test]
fn test_rendered_plain_text_contains_annotation_output() {
    let pdf = make_pdf_with_annotations();
    let config = extraction_config(OutputFormat::Plain);
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("PDF with annotations must extract successfully");

    assert!(
        result.content.contains("Annotations:"),
        "rendered plain text must contain the annotations section header; got: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains("Highlight (page 1) by Alice Reviewer: Hello World"),
        "rendered plain text must describe the Highlight with its marked text; got: {}",
        result.content
    );
}
