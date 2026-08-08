#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: org logging policy exempts tests
//! Regression coverage for issue #209.
//!
//! `ImageExtractionConfig::{target_dpi, max_image_dimension, auto_adjust_dpi,
//! min_dpi, max_dpi}` were parsed into config but never read by anything: every
//! OCR backend call site built `ImagePreprocessingConfig::default()` instead of
//! deriving it from `ExtractionConfig::images`. Paying customers set these
//! fields through the enterprise REST API/OpenAPI surface, so the values were
//! silently dropped.
//!
//! This test proves `target_dpi` / `max_image_dimension` actually reach the
//! image handed to the OCR backend by asserting on the
//! `ocr_processed_image_width` / `ocr_processed_image_height` metadata that
//! Tesseract execution already records for the *actual* pixel dimensions it
//! ran recognition on (`crates/xberg/src/ocr/processor/execution.rs`).
//!
//! The fixture is a flat mid-grey canvas (deliberately not white/bright): a
//! bright "clean page" trips Tesseract's own auto-preprocessing heuristic
//! (`should_apply_default_preprocessing` in `ocr/processor/execution.rs`),
//! which would resample the image a second time and make the asserted
//! dimensions depend on that unrelated heuristic instead of purely on
//! `ImageExtractionConfig`.

#![cfg(feature = "ocr")]

mod helpers;
use helpers::extract_bytes_document_blocking;
use xberg::core::config::{ExtractionConfig, ImageExtractionConfig, OcrConfig};

/// Flat page background. Deliberately grey (luminance well under the 0.90
/// "clean page" threshold), so Tesseract's own default-preprocessing
/// heuristic never fires and the only DPI/dimension normalization exercised
/// is the one driven by `ImageExtractionConfig` at the extractor boundary.
const PAGE_GREY: u8 = 200;
const SOURCE_WIDTH: u32 = 400;
const SOURCE_HEIGHT: u32 = 300;

/// 2x the assumed 72 DPI baseline, so the DPI scale factor is exactly 2.0 with
/// no floating-point rounding ambiguity.
const CONFIGURED_TARGET_DPI: i32 = 144;
/// Well below the 800x600-scaled intermediate size, so the resize is
/// dimension-clamped to a precisely predictable (100, 75).
const CONFIGURED_MAX_IMAGE_DIMENSION: i32 = 100;

fn grey_canvas_png() -> Vec<u8> {
    let canvas = image::RgbImage::from_pixel(SOURCE_WIDTH, SOURCE_HEIGHT, image::Rgb([PAGE_GREY; 3]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("flat grey canvas must encode as PNG");
    png
}

fn tesseract_config(images: Option<ImageExtractionConfig>) -> ExtractionConfig {
    ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        images,
        force_ocr: false,
        ..Default::default()
    }
}

/// Baseline: with no `ImageExtractionConfig`, the image reaches Tesseract
/// completely unchanged, so the processed dimensions must equal the source
/// dimensions exactly.
#[test]
fn without_images_config_ocr_processes_the_source_dimensions_unchanged() {
    let png = grey_canvas_png();
    let result = extract_bytes_document_blocking(&png, "image/png", &tesseract_config(None))
        .expect("should OCR the flat grey canvas");

    assert_eq!(
        result
            .metadata
            .additional
            .get("ocr_processed_image_width")
            .and_then(|v| v.as_u64()),
        Some(u64::from(SOURCE_WIDTH)),
        "without ImageExtractionConfig the processed width must equal the source width"
    );
    assert_eq!(
        result
            .metadata
            .additional
            .get("ocr_processed_image_height")
            .and_then(|v| v.as_u64()),
        Some(u64::from(SOURCE_HEIGHT)),
        "without ImageExtractionConfig the processed height must equal the source height"
    );
}

/// #209: `target_dpi` and `max_image_dimension` set on
/// `ExtractionConfig::images` must actually reach OCR preprocessing and change
/// the pixel dimensions Tesseract runs recognition on, to the exact clamped
/// value dictated by `max_image_dimension` — not silently be dropped in favor
/// of `ImagePreprocessingConfig::default()`.
#[test]
fn image_extraction_config_dpi_and_dimension_limit_reach_ocr_preprocessing() {
    let png = grey_canvas_png();
    let images_config = ImageExtractionConfig {
        target_dpi: CONFIGURED_TARGET_DPI,
        max_image_dimension: CONFIGURED_MAX_IMAGE_DIMENSION,
        auto_adjust_dpi: false,
        ..Default::default()
    };

    let result = extract_bytes_document_blocking(&png, "image/png", &tesseract_config(Some(images_config)))
        .expect("should OCR the DPI/dimension-normalized grey canvas");

    // target_dpi=144 (2x the assumed 72 DPI baseline) scales 400x300 to
    // 800x600, then max_image_dimension=100 clamps the longer edge to
    // exactly 100, carrying the 4:3 aspect ratio down to 75.
    assert_eq!(
        result
            .metadata
            .additional
            .get("ocr_processed_image_width")
            .and_then(|v| v.as_u64()),
        Some(100),
        "max_image_dimension=100 must clamp the processed width to exactly 100, got: {:?}",
        result.metadata.additional.get("ocr_processed_image_width")
    );
    assert_eq!(
        result
            .metadata
            .additional
            .get("ocr_processed_image_height")
            .and_then(|v| v.as_u64()),
        Some(75),
        "max_image_dimension=100 must clamp the processed height to exactly 75, got: {:?}",
        result.metadata.additional.get("ocr_processed_image_height")
    );
}
