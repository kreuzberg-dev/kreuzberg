#![cfg(feature = "pdf")]

mod helpers;
use helpers::extract_bytes_document_blocking;

use xberg::ExtractionConfig;

/// Build a minimal PDF whose Info dictionary carries NO `/Title`/`/Author`
/// at all, but whose catalog `/Metadata` stream carries an XMP packet with
/// `dc:title` and `dc:creator` (ISO 32000-1:2008 §14.3.2). This replicates
/// modern web/office exporters that populate XMP but leave the legacy Info
/// dictionary sparse or empty.
fn make_pdf_with_xmp_only_title() -> Vec<u8> {
    let content_stream = b"BT /F1 12 Tf 72 700 Td (Body text) Tj ET";

    let xmp = b"<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
                <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
                <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
                <rdf:Description rdf:about=\"\"\n\
                    xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
                <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">XMP-Only Title</rdf:li></rdf:Alt></dc:title>\n\
                <dc:creator><rdf:Seq><rdf:li>Ada Lovelace</rdf:li></rdf:Seq></dc:creator>\n\
                </rdf:Description>\n\
                </rdf:RDF>\n\
                </x:xmpmeta>\n\
                <?xpacket end=\"w\"?>";

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
    push_bytes!(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R >>\nendobj\n");

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
            /Resources << /Font << /F1 5 0 R >> >>\n\
            /Contents 4 0 R >>\n\
         endobj\n"
    );

    let off5 = pdf.len();
    push_bytes!(
        b"5 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica\n\
            /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );

    let off6 = pdf.len();
    push_str!(format!(
        "6 0 obj\n<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n",
        xmp.len()
    ));
    push_bytes!(xmp);
    push_bytes!(b"\nendstream\nendobj\n");

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

/// Issue #65: when the Info dictionary carries no `/Title`/`/Author`, xberg
/// must fall back to the document's XMP metadata (`dc:title`/`dc:creator`)
/// rather than leaving `Metadata::title`/`authors` as `None`. Before the
/// fix, XMP had zero references anywhere in xberg's PDF pipeline.
#[test]
fn test_xmp_title_and_creator_fill_empty_info_dict() {
    let pdf = make_pdf_with_xmp_only_title();
    let config = ExtractionConfig::default();
    let result = extract_bytes_document_blocking(&pdf, "application/pdf", &config)
        .expect("XMP-only PDF must extract without error");

    assert_eq!(
        result.metadata.title.as_deref(),
        Some("XMP-Only Title"),
        "title must fall back to XMP dc:title when the Info dict has none; got {:?}",
        result.metadata.title
    );
    assert_eq!(
        result.metadata.authors.as_deref(),
        Some(&["Ada Lovelace".to_string()][..]),
        "authors must fall back to XMP dc:creator when the Info dict has none; got {:?}",
        result.metadata.authors
    );
}
