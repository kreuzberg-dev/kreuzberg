#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::ExtractionConfig;

/// Build a minimal PDF with one optional-content group (OCG/layer) that is
/// OFF in the document's default configuration (`/OCProperties/D/OFF`).
///
/// Page content stream paints "Visible text" unconditionally, then wraps
/// "Hidden text" in a `/OC /OC1 BDC ... EMC` marked-content sequence tied
/// to the OCG via the page's `/Resources/Properties` dictionary. Per ISO
/// 32000-1:2008 §8.11.4, a viewer honoring `/OCProperties/D/OFF` never
/// renders "Hidden text" — issue #67 is that xberg's text extraction
/// ignored `/OCProperties` entirely and always surfaced it anyway.
fn make_pdf_with_default_off_layer() -> Vec<u8> {
    let content_stream = b"BT /F1 12 Tf 72 200 Td (Visible text) Tj ET\n\
                            /OC /OC1 BDC\n\
                            BT /F1 12 Tf 72 150 Td (Hidden text) Tj ET\n\
                            EMC";

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

    push_bytes!(b"%PDF-1.5\n");

    let off1 = pdf.len();
    push_bytes!(
        b"1 0 obj\n\
         << /Type /Catalog /Pages 2 0 R\n\
            /OCProperties << /OCGs [3 0 R] /D << /OFF [3 0 R] >> >> >>\n\
         endobj\n"
    );

    let off2 = pdf.len();
    push_bytes!(b"2 0 obj\n<< /Type /Pages /Kids [4 0 R] /Count 1 >>\nendobj\n");

    let off3 = pdf.len();
    push_bytes!(b"3 0 obj\n<< /Type /OCG /Name (HiddenLayer) >>\nendobj\n");

    let off5 = pdf.len();
    push_str!(format!("5 0 obj\n<< /Length {} >>\nstream\n", content_stream.len()));
    push_bytes!(content_stream);
    push_bytes!(b"\nendstream\nendobj\n");

    let off4 = pdf.len();
    push_bytes!(
        b"4 0 obj\n\
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300]\n\
            /Resources << /Font << /F1 6 0 R >> /Properties << /OC1 3 0 R >> >>\n\
            /Contents 5 0 R >>\n\
         endobj\n"
    );

    let off6 = pdf.len();
    push_bytes!(
        b"6 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica\n\
            /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );

    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 7\n\
         0000000000 65535 f \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n\
         {:010} 00000 n \r\n",
        off1, off2, off3, off4, off5, off6
    ));
    push_str!(format!(
        "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    pdf
}

/// Issue #67: text under a default-OFF optional-content group must not appear
/// in extracted content — it is invisible in every PDF viewer that honors
/// `/OCProperties/D/OFF`, and surfacing it produces phantom/duplicate text
/// for the common "redline layer" / "print vs. screen variant" authoring
/// pattern the pdf_oxide `optional_content` module docs call out.
#[test]
fn test_default_off_ocg_layer_excluded_from_extracted_text() {
    let pdf = make_pdf_with_default_off_layer();
    let config = ExtractionConfig::default();
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("layered PDF must extract without error");

    assert!(
        result.content.contains("Visible text"),
        "unconditionally-painted text must still be extracted; got: {:?}",
        result.content
    );
    assert!(
        !result.content.contains("Hidden text"),
        "text under a default-OFF OCG layer must NOT be extracted (issue #67); got: {:?}",
        result.content
    );
}
