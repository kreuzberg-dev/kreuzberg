#![cfg(all(feature = "pdf", feature = "ocr"))]
//! Regression test for #340: `crate::extractors::pdf::mod` must drain
//! `crate::pdf::render::take_pdf_oxide_render_warnings()` into the returned
//! document's `processing_warnings`, not just leave them sitting in the
//! per-thread capture buffer that `crate::pdf::render` owns.
//!
//! `issue_291_dropped_glyph_warning.rs` already proves the capture-and-dedup
//! logic in isolation by calling `render_pdf_page_to_png` /
//! `take_pdf_oxide_render_warnings` directly. This test instead goes through
//! the real public extraction entry point (`xberg::extract`) end to end,
//! forcing OCR so a page is actually rendered by
//! `crate::extractors::pdf::ocr::render_full_pdf_ocr_batch` (via
//! `render_page_with_safeguards`), and asserts the resulting
//! `ExtractedDocument::processing_warnings` carries the drained warning with
//! its expected `source` and message content.
//!
//! ~keep: OCR page rendering (unlike the layout-detection render path, see
//! #353) runs inline on the same async task that later drains the warnings in
//! `extractors/pdf/mod.rs` — it is never moved onto
//! `tokio::task::spawn_blocking`'s separate blocking-thread pool. That only
//! stays true if this test also stays on a single OS thread end to end, so it
//! deliberately does *not* use the shared `tests/helpers::extract_bytes_document_blocking`
//! (which drives its `Runtime::new()` multi-thread runtime, where tokio's
//! work-stealing scheduler is free to resume the task on a different worker
//! thread than the one that populated the thread-local capture buffer). Using
//! `#[tokio::test]`'s default current-thread runtime instead makes the thread
//! identity of render and drain provably the same, so this test cannot pass by
//! luck of scheduling the way a multi-thread-runtime or layout-path version
//! could.
mod helpers;

use helpers::extract_bytes_document;
use xberg::core::config::{ExtractionConfig, OcrConfig};
use xberg::pdf::render::install_pdf_render_diagnostics;

/// Build a minimal one-page PDF whose `/Resources /Font /F1` entry points at
/// object 5, a PDF string (`(NotAFontDict)`) rather than a font dictionary.
///
/// Duplicated from `build_pdf_with_malformed_font_resource` in
/// `issue_291_dropped_glyph_warning.rs` (see that file for the detailed
/// `pdf_oxide` failure-path explanation of why this is the deterministic,
/// installed-font-independent repro) rather than shared, because each
/// `tests/*.rs` file compiles to its own separate integration-test binary.
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

/// End-to-end: extracting a PDF whose only font resource is structurally
/// invalid must surface a `"pdf-render"`-sourced glyph-drop warning on the
/// returned document, not silently swallow it in the per-thread capture
/// buffer.
///
/// To make a single revert-check obvious: deleting the drain loop this test
/// covers (`crates/xberg/src/extractors/pdf/mod.rs`, the
/// `for pdf_render_warning in crate::pdf::render::take_pdf_oxide_render_warnings()`
/// loop right after `doc.processing_warnings.append(&mut pdf_extraction_warnings);`)
/// makes this test fail with an empty `processing_warnings`.
#[tokio::test]
async fn extract_surfaces_glyph_drop_warning_through_public_api() {
    // Capture is opt-in — a library must not seize the process-global `log`
    // backend on its own, so nothing arms it during extraction and without
    // this call `render_page_capturing_glyph_drops` short-circuits to a plain
    // render and the drain under test would have nothing to drain.
    assert!(
        install_pdf_render_diagnostics(),
        "no other component should own the log backend in this test binary"
    );

    let pdf = build_pdf_with_malformed_font_resource();

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: true,
        ..Default::default()
    };

    let doc = extract_bytes_document(&pdf, "application/pdf", &config)
        .await
        .expect("a page with a structurally invalid font resource must still extract successfully");

    let glyph_drop_warning = doc
        .processing_warnings
        .iter()
        .find(|warning| warning.source == "pdf-render")
        .unwrap_or_else(|| {
            panic!(
                "expected a \"pdf-render\"-sourced glyph-drop warning in processing_warnings, got: {:?}",
                doc.processing_warnings
            )
        });

    assert_eq!(
        glyph_drop_warning.source, "pdf-render",
        "glyph-drop warnings drained from pdf_oxide render capture must be sourced \"pdf-render\""
    );
    assert!(
        glyph_drop_warning.message.contains("Page 1"),
        "warning must name the affected page, got: {}",
        glyph_drop_warning.message
    );
    assert!(
        glyph_drop_warning.message.contains("Failed to parse font"),
        "warning must carry pdf_oxide's own diagnosis of the cause, got: {}",
        glyph_drop_warning.message
    );
}
