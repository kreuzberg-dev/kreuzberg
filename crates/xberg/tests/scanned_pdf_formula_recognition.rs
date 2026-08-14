//! Scanned-PDF formula recognition, end to end (issue #1385).
//!
//! A scanned page has no text layer and its OCR backend emits plain text
//! only, so formulas can come only from layout-detected regions recognized
//! by the formula model. These tests run the full pipeline — render, OCR,
//! layout detection, formula recognition — against a bilevel CCITT fixture
//! that mirrors the shape of a real scanned textbook page.
//!
//! Ignored by default: the run downloads the RT-DETR layout model and the
//! RapidLaTeXOCR model set (~310 MB combined) and needs tesseract. Run with:
//! `cargo test -p xberg --features pdf,ocr,formula-recognition --test scanned_pdf_formula_recognition -- --ignored`

#![allow(clippy::print_stdout, clippy::print_stderr)] // ~keep: test binaries print by design
#![cfg(all(feature = "pdf", feature = "ocr", feature = "formula-recognition"))]

mod helpers;
use helpers::extract_uri_document_blocking;

use std::path::PathBuf;
use xberg::core::config::layout::{FormulaModel, LayoutDetectionConfig};
use xberg::core::config::{ExtractionConfig, OcrConfig};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ocr")
        .join(name)
}

fn formula_config() -> ExtractionConfig {
    ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        layout: Some(LayoutDetectionConfig {
            formula_model: Some(FormulaModel::LatexOcr),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A scanned math page yields LaTeX formulas with page numbers and
/// PDF-point bboxes.
#[test]
#[ignore = "downloads layout + formula model weights (~310 MB) and needs tesseract"]
fn scanned_math_page_yields_latex_formulas() {
    let result = extract_uri_document_blocking(fixture("scanned_math.pdf"), None, &formula_config())
        .expect("scanned math fixture must extract");

    let formulas = &result.formulas;
    assert!(
        !formulas.is_empty(),
        "a scanned math page must produce formulas; content was: {}",
        result.content
    );
    // The fixture's headline equation contains n_0 in a fraction; accept any
    // spelling that keeps the structure.
    assert!(
        formulas.iter().any(|f| f.latex.contains("frac")),
        "expected at least one fraction among: {:?}",
        formulas.iter().map(|f| &f.latex).collect::<Vec<_>>()
    );
    for formula in formulas {
        assert_eq!(formula.page, Some(1), "single-page fixture");
        assert!(formula.bbox.is_some(), "PDF OCR formulas carry a bbox");
        assert!(
            !formula.latex.contains('Ġ'),
            "byte-level markers must not reach output: {}",
            formula.latex
        );
    }
}

/// A scanned page without mathematics yields no formulas under the same
/// configuration.
#[test]
#[ignore = "downloads layout + formula model weights (~310 MB) and needs tesseract"]
fn scanned_prose_page_yields_no_formulas() {
    let result = extract_uri_document_blocking(fixture("scanned_hello.pdf"), None, &formula_config())
        .expect("scanned prose fixture must extract");

    assert!(
        result.formulas.is_empty(),
        "a scanned page with no mathematics must yield no formulas, got: {:?}",
        result.formulas.iter().map(|f| &f.latex).collect::<Vec<_>>()
    );
}
