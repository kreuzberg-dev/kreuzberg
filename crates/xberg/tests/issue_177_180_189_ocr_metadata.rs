#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: org logging policy exempts tests
//! Integration coverage for the Tesseract OCR execution-path fixes:
//!
//! - #177: table detection now clusters words into independent spatial regions
//!   instead of hardcoding `table_count`/`tables_detected` to at most one
//!   whole-page table. This test pins the metadata for a real single-table
//!   image so the refactor is proven not to regress the common case (no
//!   available fixture exercises two independent tables on one page).
//! - #180: words whose parent Tesseract block is a non-text block (noise, an
//!   embedded image region, a ruling line) are no longer dropped from
//!   `ocr_elements`; they are retained, tagged with their real `block_type`,
//!   and counted into the `non_text_block_word_count` metadata key.
//! - #189: `ResultIterator::extract_all_words` now forwards
//!   `TessResultIteratorWordRecognitionLanguage` per word, surfaced as
//!   `word_language` in each `OcrElement.backend_metadata`.
//! - #175/#191: block type, justification, and crown/list-item paragraph
//!   metadata are present on every word-level `OcrElement`.

#![cfg(feature = "ocr")]

mod helpers;
use std::path::Path;

use helpers::*;
use xberg::core::config::{ExtractionConfig, OcrConfig, OutputFormat};

fn tesseract_eng_config(output_format: OutputFormat) -> ExtractionConfig {
    ExtractionConfig {
        output_format,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: false,
        ..Default::default()
    }
}

/// #177: table-region clustering must still report exactly one table (not
/// zero, and not a hardcoded miscount) for a real single-table image, with
/// row/column counts matching the actual reconstructed grid.
#[test]
fn single_table_image_reports_consistent_table_metadata() {
    if skip_if_missing("images/simple_table.png") {
        return;
    }
    let file_path = get_test_file_path("images/simple_table.png");
    let result = extract_uri_document_blocking(&file_path, None, &tesseract_eng_config(OutputFormat::Markdown))
        .expect("should extract simple_table.png with OCR table detection");

    let additional = &result.metadata.additional;

    assert_eq!(
        additional.get("table_count").and_then(|v| v.as_u64()),
        Some(1),
        "expected exactly one detected table region, got: {:?}",
        additional.get("table_count")
    );
    assert_eq!(
        additional.get("tables_detected").and_then(|v| v.as_u64()),
        Some(1),
        "table_count and tables_detected must agree"
    );
    assert_eq!(
        additional.get("table_rows").and_then(|v| v.as_u64()),
        Some(5),
        "expected the reconstructed table to keep its 5 rows"
    );
    assert_eq!(
        additional.get("table_cols").and_then(|v| v.as_u64()),
        Some(4),
        "expected the reconstructed table to keep its 4 columns"
    );
}

/// #189: every word-level `OcrElement` from a single-language `eng` OCR run
/// must report `word_language: "eng"` in `backend_metadata`, not omit it.
#[test]
fn word_language_is_forwarded_per_ocr_element() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let file_path = get_test_file_path("images/test_hello_world.png");
    let result = extract_uri_document_blocking(&file_path, None, &tesseract_eng_config(OutputFormat::Plain))
        .expect("should extract test_hello_world.png with OCR");

    let elements = result.ocr_elements.expect("OCR should produce word-level elements");
    assert_eq!(elements.len(), 2, "expected exactly the two words 'Hello' and 'World'");

    for element in &elements {
        assert_eq!(
            element.backend_metadata.get("word_language"),
            Some(&serde_json::json!("eng")),
            "word {:?} should report its recognition language",
            element.text
        );
        assert_eq!(
            element.backend_metadata.get("block_type"),
            Some(&serde_json::json!("PT_FLOWING_TEXT")),
            "word {:?} should carry its Tesseract block type (#175)",
            element.text
        );
        assert_eq!(
            element.backend_metadata.get("is_crown"),
            Some(&serde_json::json!(false)),
            "word {:?} should carry paragraph crown metadata (#191)",
            element.text
        );
    }

    let texts: Vec<&str> = elements.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, vec!["Hello", "World"]);
}

// ---------------------------------------------------------------------------
// #180: words inside non-text (image / noise / ruling-line) blocks
// ---------------------------------------------------------------------------
//
// No shipped fixture makes Tesseract place a *recognised word* inside a
// non-text block, so this test composes one deterministically from the
// existing `test_hello_world.png` fixture:
//
//   * an 800x600 flat mid-grey page (grey, not white, so the extractor takes
//     the raw pass-through branch instead of the DPI-normalise + Leptonica
//     branch — that keeps the rendered pixels, and therefore Tesseract's
//     layout analysis, fully deterministic),
//   * the two words "Hello" and "World" cropped out of the fixture and
//     blended onto the grey page,
//   * a black diagonal band drawn across the page. Tesseract classifies the
//     band as `PT_FLOWING_IMAGE`, and because the band is diagonal its
//     *bounding box* spans the words, which the block lookup resolves before
//     it reaches the words' own text block.
//
// Every constant below is load-bearing for that layout; changing one changes
// what Tesseract segments.
//
// The page segmentation mode is load-bearing too, which is why these tests use
// [`tesseract_full_page_config`] instead of [`tesseract_eng_config`]: for a
// bare image with no explicit `tesseract_config`, `ImageExtractor` overrides
// PSM to 11 ("sparse text, no OSD"). PSM 11 skips full page layout analysis
// altogether, so Tesseract reports every word as `PT_FLOWING_TEXT` and the
// non-text-block path in `ocr/processor/execution.rs` is unreachable. Only the
// full-auto PSM 3 path — used by PDF page OCR and by any caller that supplies
// its own `tesseract_config` — classifies polyblocks at all.

/// Page background. Deliberately grey, not white: a white page trips the
/// "clean page" heuristic and routes the image through DPI normalisation and
/// Leptonica preprocessing, which would resample the pixels asserted on here.
const PAGE_GREY: u8 = 200;
const CANVAS_WIDTH: u32 = 800;
const CANVAS_HEIGHT: u32 = 600;

/// Tight crop of "Hello World" inside `images/test_hello_world.png`.
const TEXT_CROP_X: u32 = 210;
const TEXT_CROP_Y: u32 = 67;
const TEXT_CROP_WIDTH: u32 = 381;
const TEXT_CROP_HEIGHT: u32 = 73;

/// Where the crop lands on the canvas — inside the diagonal band's bbox.
const TEXT_PASTE_X: u32 = 90;
const TEXT_PASTE_Y: u32 = 380;

/// Pixels at least this bright in every channel are the fixture's white paper
/// and are repainted to [`PAGE_GREY`] so the crop blends into the page instead
/// of forming a white rectangle of its own.
const LIGHT_PIXEL_MIN: u8 = 153;

/// The black diagonal band: a parallelogram from (40, 40) to (760, 540).
const BAND_TOP_Y: u32 = 40;
const BAND_BOTTOM_Y: u32 = 540;
const BAND_TOP_LEFT_X: u32 = 40;
const BAND_BOTTOM_LEFT_X: u32 = 660;
const BAND_WIDTH: u32 = 100;

/// Tesseract's fully-automatic page segmentation mode. Supplying an explicit
/// `tesseract_config` is what stops `ImageExtractor` from forcing PSM 11 on a
/// bare image; it also stops the extractor from implicitly requesting
/// word-level elements, so `element_config` has to ask for them by hand.
const FULL_PAGE_SEGMENTATION_PSM: i32 = 3;

/// OCR config that keeps Tesseract's polyblock classification alive, so words
/// can actually be parented to a non-text block.
fn tesseract_full_page_config(output_format: OutputFormat) -> ExtractionConfig {
    ExtractionConfig {
        output_format,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            tesseract_config: Some(xberg::TesseractConfig {
                language: vec!["eng".to_string()],
                psm: FULL_PAGE_SEGMENTATION_PSM,
                ..Default::default()
            }),
            element_config: Some(xberg::OcrElementConfig {
                include_elements: true,
                min_level: xberg::OcrElementLevel::Word,
                ..Default::default()
            }),
            ..Default::default()
        }),
        force_ocr: false,
        ..Default::default()
    }
}

/// Compose the PNG described above and return its encoded bytes.
///
/// `draw_graphic_band` selects between the two halves of the A/B pair: with the
/// band the words land in a `PT_FLOWING_IMAGE` block, without it the identical
/// words land in an ordinary text block.
fn text_page_png(word_fixture: &Path, draw_graphic_band: bool) -> Vec<u8> {
    let source = image::open(word_fixture)
        .expect("test_hello_world.png fixture must decode")
        .to_rgb8();

    let mut canvas = image::RgbImage::from_pixel(CANVAS_WIDTH, CANVAS_HEIGHT, image::Rgb([PAGE_GREY; 3]));

    for row in 0..TEXT_CROP_HEIGHT {
        for column in 0..TEXT_CROP_WIDTH {
            let [red, green, blue] = source.get_pixel(TEXT_CROP_X + column, TEXT_CROP_Y + row).0;
            let blended = if red >= LIGHT_PIXEL_MIN && green >= LIGHT_PIXEL_MIN && blue >= LIGHT_PIXEL_MIN {
                image::Rgb([PAGE_GREY; 3])
            } else {
                image::Rgb([red, green, blue])
            };
            canvas.put_pixel(TEXT_PASTE_X + column, TEXT_PASTE_Y + row, blended);
        }
    }

    if draw_graphic_band {
        let vertical_span = BAND_BOTTOM_Y - BAND_TOP_Y;
        let horizontal_run = BAND_BOTTOM_LEFT_X - BAND_TOP_LEFT_X;
        for y in BAND_TOP_Y..=BAND_BOTTOM_Y {
            let left_edge = BAND_TOP_LEFT_X + horizontal_run * (y - BAND_TOP_Y) / vertical_span;
            for x in left_edge..=(left_edge + BAND_WIDTH) {
                canvas.put_pixel(x, y, image::Rgb([0, 0, 0]));
            }
        }
    }

    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("composed fixture must encode as PNG");
    png
}

/// #180: a word whose parent block is `PT_FLOWING_IMAGE` must survive into
/// `ocr_elements` carrying that block type, and must be counted in
/// `non_text_block_word_count`.
///
/// The retained-word count is asserted exactly, not just as "more than zero":
/// `min_confidence` defaults to `0.0`, so keeping non-text-block words also
/// opens the door to Tesseract hallucinating over graphics. Tesseract emits
/// four raw words for this page — the two real ones plus two whitespace-only
/// artefacts of the black band — and only the two real words may reach the
/// caller.
#[test]
fn should_retain_and_tag_words_when_parent_block_is_a_graphic_region() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let png = text_page_png(&get_test_file_path("images/test_hello_world.png"), true);

    let result = extract_bytes_document_blocking(&png, "image/png", &tesseract_full_page_config(OutputFormat::Plain))
        .expect("should OCR the composed text-inside-graphic page");

    let elements = result
        .ocr_elements
        .expect("OCR should produce word-level elements for the composed page");

    let texts: Vec<&str> = elements.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["Hello", "World"],
        "exactly the two real words must be retained; anything extra means \
         min_confidence=0.0 is now admitting hallucinated words over the graphic"
    );

    for element in &elements {
        assert_eq!(
            element.backend_metadata.get("block_type"),
            Some(&serde_json::json!("PT_FLOWING_IMAGE")),
            "word {:?} must be tagged with its real non-text block type, not silently dropped",
            element.text
        );
    }

    assert_eq!(
        result
            .metadata
            .additional
            .get("non_text_block_word_count")
            .and_then(|v| v.as_u64()),
        Some(2),
        "both retained words sit in a non-text block, so the counter must read exactly 2; got {:?}",
        result.metadata.additional.get("non_text_block_word_count")
    );
}

/// #180 control, and the other half of the A/B pair: the *same* page and the
/// *same* two words, with the diagonal graphic removed. The words must then be
/// parented to an ordinary text block, and `non_text_block_word_count` must be
/// ABSENT — not present with a value of `0` — so callers can use key presence
/// as the "OCR read text off a graphic" signal.
///
/// Removing the graphic is the only difference from
/// `should_retain_and_tag_words_when_parent_block_is_a_graphic_region`, so a
/// change that makes every word look non-text (or every word look text) breaks
/// exactly one of the pair.
#[test]
fn should_omit_non_text_block_word_count_when_every_word_is_in_a_text_block() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let png = text_page_png(&get_test_file_path("images/test_hello_world.png"), false);

    let result = extract_bytes_document_blocking(&png, "image/png", &tesseract_full_page_config(OutputFormat::Plain))
        .expect("should OCR the composed graphic-free page");

    let elements = result
        .ocr_elements
        .expect("OCR should produce word-level elements for the composed page");
    let texts: Vec<&str> = elements.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["Hello", "World"],
        "the control page must yield the same two words as the graphic page"
    );

    for element in &elements {
        assert_eq!(
            element.backend_metadata.get("block_type"),
            Some(&serde_json::json!("PT_FLOWING_TEXT")),
            "without the graphic, word {:?} must be parented to an ordinary text block",
            element.text
        );
    }

    assert_eq!(
        result.metadata.additional.get("non_text_block_word_count"),
        None,
        "a page whose words all sit in text blocks must not carry the key at all, got {:?}",
        result.metadata.additional.get("non_text_block_word_count")
    );
}
