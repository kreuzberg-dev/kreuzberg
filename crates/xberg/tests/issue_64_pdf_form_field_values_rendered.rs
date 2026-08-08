#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::ExtractionConfig;
use xberg::core::config::OutputFormat;

/// Build a minimal PDF whose form field values live ONLY in Widget
/// annotations — the interactive (non-flattened) pattern where the content
/// stream never mentions the entered value at all. Mirrors
/// `pdf_form_fields.rs::make_interactive_form_pdf`.
fn make_interactive_form_pdf() -> Vec<u8> {
    let content_stream = b"BT /Helvetica 12 Tf 72 700 Td (Name:) Tj 0 -30 Td (Email:) Tj ET";

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
    push_bytes!(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>\nendobj\n");

    let off2 = pdf.len();
    push_bytes!(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    let off4 = pdf.len();
    push_str!(format!("4 0 obj\n<< /Length {} >>\nstream\n", content_stream.len()));
    push_bytes!(content_stream);
    push_bytes!(b"\nendstream\nendobj\n");

    let off3 = pdf.len();
    push_bytes!(
        b"3 0 obj\n\
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]\n\
            /Contents 4 0 R\n\
            /Resources << /Font << /Helvetica 8 0 R >> >>\n\
            /Annots [6 0 R 7 0 R] >>\n\
         endobj\n"
    );

    let off5 = pdf.len();
    push_bytes!(
        b"5 0 obj\n\
         << /Type /AcroForm /Fields [6 0 R 7 0 R]\n\
            /DA (/Helvetica 12 Tf 0 g) >>\n\
         endobj\n"
    );

    let off6 = pdf.len();
    push_bytes!(
        b"6 0 obj\n\
         << /Type /Annot /Subtype /Widget /FT /Tx\n\
            /T (name) /V (John Smith)\n\
            /Rect [140 680 400 700] /P 3 0 R\n\
            /DA (/Helvetica 12 Tf 0 g) >>\n\
         endobj\n"
    );

    let off7 = pdf.len();
    push_bytes!(
        b"7 0 obj\n\
         << /Type /Annot /Subtype /Widget /FT /Tx\n\
            /T (email) /V (john@example.com)\n\
            /Rect [140 650 400 670] /P 3 0 R\n\
            /DA (/Helvetica 12 Tf 0 g) >>\n\
         endobj\n"
    );

    let off8 = pdf.len();
    push_bytes!(
        b"8 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica\n\
            /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );

    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 9\n\
         0000000000 65535 f \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n",
        off1, off2, off3, off4, off5, off6, off7, off8
    ));
    push_str!(format!(
        "trailer\n<< /Size 9 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    pdf
}

/// Issue #64: `doc.form_fields` reaches `ExtractedDocument.form_fields`, but
/// before the fix nothing rendered it into `content` for structured output
/// formats. The plain-text path already got these values via a separate
/// Widget-splice mechanism (`oxide::text::append_missing_widget_values`,
/// issue #1120) — this test targets Markdown specifically, which is built
/// from `pdf::oxide::hierarchy` span segments that mechanism never touches.
#[test]
fn test_interactive_form_field_values_appear_in_markdown_output() {
    let pdf = make_interactive_form_pdf();
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("interactive form PDF must extract without error");

    assert!(
        result.content.contains("John Smith"),
        "Markdown output must include Widget field value 'John Smith'; got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("john@example.com"),
        "Markdown output must include Widget field value 'john@example.com'; got: {:?}",
        result.content
    );
}

/// A value that is already present in the rendered content (e.g. because a
/// flattened form's appearance was painted into the content stream) must not
/// be duplicated by the injector.
#[test]
fn test_flattened_form_value_not_duplicated_in_markdown_output() {
    let content_stream = b"BT /Helvetica 12 Tf 72 700 Td (Name: Jane Doe) Tj ET";

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
    push_bytes!(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>\nendobj\n");
    let off2 = pdf.len();
    push_bytes!(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    let off4 = pdf.len();
    push_str!(format!("4 0 obj\n<< /Length {} >>\nstream\n", content_stream.len()));
    push_bytes!(content_stream);
    push_bytes!(b"\nendstream\nendobj\n");
    let off3 = pdf.len();
    push_bytes!(
        b"3 0 obj\n\
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]\n\
            /Contents 4 0 R\n\
            /Resources << /Font << /Helvetica 7 0 R >> >>\n\
            /Annots [6 0 R] >>\n\
         endobj\n"
    );
    let off5 = pdf.len();
    push_bytes!(b"5 0 obj\n<< /Type /AcroForm /Fields [6 0 R] /DA (/Helvetica 12 Tf 0 g) >>\nendobj\n");
    let off6 = pdf.len();
    push_bytes!(
        b"6 0 obj\n\
         << /Type /Annot /Subtype /Widget /FT /Tx\n\
            /T (name) /V (Jane Doe)\n\
            /Rect [140 680 400 700] /P 3 0 R\n\
            /DA (/Helvetica 12 Tf 0 g) >>\n\
         endobj\n"
    );
    let off7 = pdf.len();
    push_bytes!(
        b"7 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica\n\
            /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );
    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 8\n\
         0000000000 65535 f \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n",
        off1, off2, off3, off4, off5, off6, off7
    ));
    push_str!(format!(
        "trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };
    let result =
        extract_bytes_document_blocking(&pdf, "application/pdf", &config).expect("flattened form PDF must extract");

    let count = result.content.matches("Jane Doe").count();
    assert_eq!(
        count, 1,
        "flattened Widget value 'Jane Doe' must appear exactly once in Markdown output, not \
         duplicated by the field-value injector; got: {:?}",
        result.content
    );
}
