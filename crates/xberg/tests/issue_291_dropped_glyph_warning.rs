#![cfg(feature = "pdf")]

use xberg::ProcessingWarning;
use xberg::pdf::render::{install_pdf_render_diagnostics, render_pdf_page_to_png, take_pdf_oxide_render_warnings};

/// Build a minimal one-page PDF whose `/Resources /Font /F1` entry points at
/// object 5, which is a PDF string (`(NotAFontDict)`) rather than a font
/// dictionary. `pdf_oxide`'s `PageRenderer::load_resources`
/// (`page_renderer.rs`) resolves every `/Font` resource through
/// `PdfDocument::get_or_load_font_for_rendering`, which requires the
/// resolved object to be a dictionary; here it fails with `Error::ParseError
/// { reason: "Font object is not a dictionary" }`.
///
/// Per issue #1364, that failure does not abort the page: `load_resources`
/// logs it via `log::warn!("Failed to parse font '{}': {}. Text using this
/// font may render incorrectly.", ...)` and continues, so every glyph the
/// content stream later paints with `/F1` is dropped (the renderer has no
/// font to shape it with) while the page still renders `Ok`. Unlike a
/// missing font *name* (which `pdf_oxide` best-effort substitutes via
/// `fontdb` system-font matching and may or may not warn about depending on
/// which fonts happen to be installed), a structurally invalid font
/// **resource** always fails to parse regardless of the host's installed
/// fonts, making this the deterministic repro for CI.
fn build_pdf_with_malformed_font_resource() -> Vec<u8> {
    let content_stream = b"BT /F1 24 Tf 72 700 Td (Hello) Tj 0 -40 Td (World) Tj ET";

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
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 792]\n\
            /Resources << /Font << /F1 5 0 R >> >>\n\
            /Contents 4 0 R >>\n\
         endobj\n"
    );

    let off5 = pdf.len();
    push_bytes!(b"5 0 obj\n(NotAFontDict)\nendobj\n");

    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 6\n\
         0000000000 65535 f \r\n\
         {off1:010} 00000 n \r\n\
         {off2:010} 00000 n \r\n\
         {off3:010} 00000 n \r\n\
         {off4:010} 00000 n \r\n\
         {off5:010} 00000 n \r\n"
    ));
    push_str!(format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    pdf
}

/// Build a minimal one-page PDF whose content stream uses a properly declared
/// base-14 font (`/Helvetica`), used as a negative control: rendering a page
/// with a resolvable font must not produce a glyph-drop warning.
fn build_pdf_with_resolvable_font() -> Vec<u8> {
    let content_stream = b"BT /F1 24 Tf 72 700 Td (Hello) Tj ET";

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
    push_bytes!(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    let off2 = pdf.len();
    push_bytes!(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    let off5 = pdf.len();
    push_str!(format!("5 0 obj\n<< /Length {} >>\nstream\n", content_stream.len()));
    push_bytes!(content_stream);
    push_bytes!(b"\nendstream\nendobj\n");

    let off3 = pdf.len();
    push_bytes!(
        b"3 0 obj\n\
         << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 792]\n\
            /Resources << /Font << /F1 4 0 R >> >>\n\
            /Contents 5 0 R >>\n\
         endobj\n"
    );

    let off4 = pdf.len();
    push_bytes!(
        b"4 0 obj\n\
         << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\n\
         endobj\n"
    );

    let xref_off = pdf.len();
    push_str!(format!(
        "xref\n0 6\n\
         0000000000 65535 f \r\n\
         {off1:010} 00000 n \r\n\
         {off2:010} 00000 n \r\n\
         {off3:010} 00000 n \r\n\
         {off4:010} 00000 n \r\n\
         {off5:010} 00000 n \r\n"
    ));
    push_str!(format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n"
    ));

    pdf
}

/// Issue #291 / GH#1364: a page whose `/Font` resource is structurally
/// invalid renders successfully (as it must — a broken font resource must
/// not fail the whole page) but drops every glyph drawn with it. Before this
/// fix nothing surfaced that: `RenderedImage` has no diagnostic field, and
/// pdf_oxide's own `log::warn!` call had no `log::Log` backend installed to
/// receive it, so the drop was invisible twice over.
///
/// This asserts the xberg-side capture end to end: rendering the *same* page
/// twice (e.g. once for a preview and once for OCR, or a retry) hits the
/// identical pdf_oxide cause both times, so the two `log::warn!` records
/// must dedup into exactly one `ProcessingWarning`, sourced `"pdf-render"`
/// and naming the page — not one warning per render call.
#[test]
fn test_dropped_glyphs_from_malformed_font_produce_one_deduped_warning() {
    // Capture is opt-in — a library must not seize the process-global `log`
    // backend on its own. An application (here, the test) asks for it.
    assert!(
        install_pdf_render_diagnostics(),
        "no other component should own the log backend in this test binary"
    );

    // Drain any residual state from a previous render on this thread so the
    // assertion below is exact, not "at least".
    let _ = take_pdf_oxide_render_warnings();

    let pdf = build_pdf_with_malformed_font_resource();

    let first_png = render_pdf_page_to_png(&pdf, 0, Some(150), None)
        .expect("page with a malformed font resource must still render");
    assert!(!first_png.is_empty(), "renderer must still produce page bytes");

    let second_png =
        render_pdf_page_to_png(&pdf, 0, Some(150), None).expect("re-rendering the same page must also succeed");
    assert!(!second_png.is_empty(), "renderer must still produce page bytes");

    let warnings = take_pdf_oxide_render_warnings();
    assert_eq!(
        warnings.len(),
        1,
        "two renders hitting the identical malformed-font cause must dedup to one warning, got: {warnings:?}"
    );

    let warning = &warnings[0];
    assert_eq!(
        warning.source, "pdf-render",
        "glyph-drop warnings must be sourced \"pdf-render\", got: {}",
        warning.source
    );
    assert!(
        warning.message.contains("Page 1"),
        "warning must name the affected page, got: {}",
        warning.message
    );
    assert!(
        warning.message.contains("glyph"),
        "warning must describe glyph loss, not just repeat the raw pdf_oxide log line, got: {}",
        warning.message
    );
    assert!(
        warning.message.contains("Failed to parse font"),
        "warning must carry pdf_oxide's own diagnosis of the cause, got: {}",
        warning.message
    );

    // A second drain on the same thread must come back empty: warnings are
    // consumed, not accumulated forever across unrelated render calls.
    let drained_again = take_pdf_oxide_render_warnings();
    assert!(
        drained_again.is_empty(),
        "take_pdf_oxide_render_warnings must drain, not peek: got {drained_again:?}"
    );
}

/// Negative control: a page whose font resolves cleanly (a declared base-14
/// font) must not produce a glyph-drop warning. Without this, a change that
/// makes the capture logger too broad (e.g. matching on level alone) would
/// pass the positive test above while flooding every normal render with
/// spurious warnings.
#[test]
fn test_resolvable_font_page_produces_no_glyph_drop_warning() {
    // Install first, otherwise "no warnings" would pass trivially because
    // capture was never armed — a test that passes when the code is broken.
    assert!(
        install_pdf_render_diagnostics(),
        "no other component should own the log backend in this test binary"
    );
    let _ = take_pdf_oxide_render_warnings();

    let pdf = build_pdf_with_resolvable_font();
    let png = render_pdf_page_to_png(&pdf, 0, Some(150), None).expect("page with a resolvable font must render");
    assert!(!png.is_empty(), "renderer must still produce page bytes");

    let warnings: Vec<ProcessingWarning> = take_pdf_oxide_render_warnings();
    assert!(
        warnings.is_empty(),
        "a page whose font resolves cleanly must not produce a glyph-drop warning, got: {warnings:?}"
    );
}
