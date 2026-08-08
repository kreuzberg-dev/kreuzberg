#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::ExtractionConfig;

/// Build a PDF whose catalog `/AcroForm` entry is present but malformed: it
/// points to an integer, not a dictionary. `pdf_oxide::extractors::forms::
/// FormExtractor::extract_fields` returns `Err(InvalidPdf("AcroForm is not
/// a dictionary"))` for this — a genuine parse failure, not the (very
/// common) "no AcroForm at all" case that returns `Ok(vec![])`.
fn make_pdf_with_malformed_acroform() -> Vec<u8> {
    let content_stream = b"BT /F1 12 Tf 72 700 Td (Body text) Tj ET";

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
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300]\n\
            /Resources << /Font << /F1 6 0 R >> >>\n\
            /Contents 4 0 R >>\n\
         endobj\n"
    );

    // /AcroForm points at an integer, not a dictionary — this is what makes
    // extraction fail rather than merely finding "no form".
    let off5 = pdf.len();
    push_bytes!(b"5 0 obj\n42\nendobj\n");

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

/// Issue #73: a malformed `/AcroForm` (present but not a dictionary) must
/// surface `ProcessingWarning`s with source `pdf_forms` — not just internal
/// `tracing::debug!` lines — so a form-bearing-but-broken PDF is
/// distinguishable from a PDF with no form at all. `result.form_fields`
/// stays empty either way; the warnings are the only observable signal.
///
/// Both the AcroForm-field extractor and the XFA extractor independently
/// resolve `/AcroForm` and both hit the same malformed object, so a fully
/// broken `/AcroForm` legitimately produces two warnings, one per layer.
#[test]
fn test_malformed_acroform_produces_processing_warnings() {
    let pdf = make_pdf_with_malformed_acroform();
    let config = ExtractionConfig::default();
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("PDF with malformed /AcroForm must still extract (degraded, not fatal)");

    assert!(
        result.form_fields.is_empty(),
        "a malformed AcroForm cannot yield any fields; got {:?}",
        result.form_fields
    );

    let form_warnings: Vec<_> = result
        .processing_warnings
        .iter()
        .filter(|w| w.source.as_ref() == "pdf_forms")
        .collect();
    assert_eq!(
        form_warnings.len(),
        2,
        "one AcroForm-layer warning and one XFA-layer warning must be recorded \
         (both independently resolve the same malformed /AcroForm); got {:?}",
        result.processing_warnings
    );
    assert!(
        form_warnings
            .iter()
            .any(|w| w.message.contains("AcroForm field extraction failed")),
        "must include the AcroForm-layer failure warning; got {form_warnings:?}"
    );
    assert!(
        form_warnings.iter().any(|w| w
            .message
            .contains("XFA form data was present but could not be extracted")),
        "must include the XFA-layer failure warning; got {form_warnings:?}"
    );
}
