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

#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: org logging policy exempts tests
#![cfg(feature = "ocr")]

mod helpers;
use std::path::Path;
use std::sync::{Arc, Mutex};

use helpers::*;
use tracing_subscriber::layer::SubscriberExt as _;
use xberg::core::config::{ExtractionConfig, OcrConfig, OutputFormat};

/// Captures the fields of the `extract_all_paragraphs` skip-diagnosis event
/// emitted by `ocr::processor::execution::extract_elements_via_iterator`
/// (`crates/xberg/src/ocr/processor/execution.rs`), so a CI failure can show
/// *why* per-word paragraph metadata (`is_crown`/`is_list_item`/`justification`)
/// is missing without needing `RUST_LOG=warn` set on the runner.
///
/// Field names must track the `tracing::warn!` call site exactly: `skipped_no_para_info`,
/// `skipped_no_bbox`, and `extracted_paragraphs`.
#[derive(Debug, Clone, Default)]
struct ParagraphSkipEvent {
    skipped_no_para_info: Option<u64>,
    skipped_no_bbox: Option<u64>,
    extracted_paragraphs: Option<u64>,
}

impl ParagraphSkipEvent {
    /// An event is "relevant" once it carries at least one of the three
    /// diagnosis fields; this filters out unrelated `tracing` events that may
    /// fire during the same extraction call.
    fn is_relevant(&self) -> bool {
        self.skipped_no_para_info.is_some() || self.skipped_no_bbox.is_some() || self.extracted_paragraphs.is_some()
    }
}

impl tracing::field::Visit for ParagraphSkipEvent {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        match field.name() {
            "skipped_no_para_info" => self.skipped_no_para_info = Some(value),
            "skipped_no_bbox" => self.skipped_no_bbox = Some(value),
            "extracted_paragraphs" => self.extracted_paragraphs = Some(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
        // every other field (e.g. the event's `message`) is intentionally ignored.
    }
}

/// A `tracing` `Layer` that records every emitted event carrying one of the
/// paragraph-skip diagnosis fields.
///
/// ★ This MUST be a process-global default subscriber, not the thread-local
/// `tracing::subscriber::with_default` used by `tests/api_logging.rs`.
///
/// The event being captured is emitted by `perform_ocr`
/// (`ocr/processor/execution.rs`), which Tesseract reaches through
/// `processor.process_image(..)` inside the closure that
/// `ocr/tesseract_backend.rs` hands to `tokio::task::spawn_blocking`. That runs
/// on a blocking-pool worker thread, and `tracing`'s `with_default` override is
/// thread-local — so a scoped subscriber installed on the test thread never
/// sees it and captures nothing at all.
///
/// This is safe here and does NOT repeat the `tracing-interest-cache-poisoning`
/// incident in project memory: that was caused by calling
/// `rebuild_interest_cache()` by hand outside `with_default`, whereas
/// `set_global_default` rebuilds the callsite interest cache itself, so the
/// diagnostic still fires even if another test in this binary evaluated the
/// callsite first. Each integration-test file is its own binary, so the global
/// default is scoped to this file's tests. ~keep
#[derive(Clone, Default)]
struct ParagraphSkipCapture {
    events: Arc<Mutex<Vec<ParagraphSkipEvent>>>,
}

/// Installs the capture layer as the process-global default exactly once and
/// returns a handle to the shared buffer.
fn install_paragraph_skip_capture() -> ParagraphSkipCapture {
    static CAPTURE: std::sync::OnceLock<ParagraphSkipCapture> = std::sync::OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = ParagraphSkipCapture::default();
            let subscriber = tracing_subscriber::registry().with(capture.clone());
            // A global default can only be set once per process; ignore a
            // late loss of the race, the winner's buffer is the one returned.
            let _ = tracing::subscriber::set_global_default(subscriber);
            capture
        })
        .clone()
}

impl<S> tracing_subscriber::Layer<S> for ParagraphSkipCapture
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = ParagraphSkipEvent::default();
        event.record(&mut visitor);
        if visitor.is_relevant() {
            self.events.lock().unwrap().push(visitor);
        }
    }
}

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
/// whose recognition language Tesseract chooses to report must forward it as
/// `word_language: "eng"` in `backend_metadata`.
///
/// Presence is not asserted unconditionally: `WordData::language`
/// (`xberg-tesseract/src/result_iterator.rs`) is populated from
/// `TessResultIteratorWordRecognitionLanguage`, and that call's own doc
/// comment records that a null return ("Tesseract could not attribute this
/// word to a specific language") is a normal, non-fatal outcome, not an
/// error -- `extract_word_data_unlocked` maps it to `None` rather than
/// failing the extraction. This was observed to happen for this exact
/// fixture (PSM 11, `eng`-only) on x86_64-linux CI only, while
/// ubuntu-24.04-arm and macos-latest both report the language -- the same
/// x86_64-linux-only FFI-declination pattern already documented and handled
/// for `is_crown`/`is_list_item`/`justification` below (see #180/#191's
/// paragraph-metadata comment and commit 727c2df67d). Since production code
/// already treats a missing language as legitimate, a test that hard-asserts
/// its presence enforces a stronger contract than the code itself promises,
/// so it is pinned only when present.
///
/// Coverage of the forwarding itself is NOT lost by that relaxation, which is
/// what makes it safe rather than a vacuous test: `ocr/conversion.rs`'s
/// `iterator_word_to_element_forwards_underline_font_id_crown_indent_and_language`
/// builds a `WordData { language: Some("deu"), .. }` directly and asserts the
/// resulting `word_language` metadata equals `"deu"`, with no Tesseract call
/// and so no platform dependence. If the forwarding in
/// `iterator_word_to_element` ever breaks, that unit test fails on every
/// architecture. This integration test covers only whether Tesseract chose to
/// report a language at runtime, which is its prerogative, not our contract.
#[test]
fn word_language_is_forwarded_per_ocr_element() {
    if skip_if_missing("images/test_hello_world.png") {
        return;
    }
    let file_path = get_test_file_path("images/test_hello_world.png");

    let capture = install_paragraph_skip_capture();
    capture.events.lock().unwrap().clear();
    let result = extract_uri_document_blocking(&file_path, None, &tesseract_eng_config(OutputFormat::Plain))
        .expect("should extract test_hello_world.png with OCR");
    let captured_paragraph_skip_events = capture.events.lock().unwrap().clone();

    let elements = result.ocr_elements.expect("OCR should produce word-level elements");
    assert_eq!(elements.len(), 2, "expected exactly the two words 'Hello' and 'World'");

    for element in &elements {
        if let Some(word_language) = element.backend_metadata.get("word_language") {
            assert_eq!(
                word_language,
                &serde_json::json!("eng"),
                "word {:?} reported a recognition language but with an unexpected value",
                element.text
            );
        }
        assert_eq!(
            element.backend_metadata.get("block_type"),
            Some(&serde_json::json!("PT_FLOWING_TEXT")),
            "word {:?} should carry its Tesseract block type (#175)",
            element.text
        );
        // Paragraph metadata (is_crown / is_list_item / justification) is all-or-nothing:
        // `ocr/conversion.rs::iterator_word_to_element` writes all three from the same
        // `Option<&ParaInfo>`, resolved in `ocr/processor/execution.rs::extract_elements_via_iterator`
        // by matching this word's centroid against the RIL_PARA bboxes collected by
        // `xberg-tesseract::page_iterator::extract_all_paragraphs`. That match is observed to fail
        // on x86_64-linux CI only (ubuntu-24.04-arm and macos-latest both keep the metadata for this
        // fixture), with the identical `tesseract-ocr` apt package pinned by
        // `scripts/ci/install-system-deps/install-linux.sh` on both Linux architectures and the
        // identical PSM 11 ("sparse text, no OSD" -- this test does not supply `tesseract_config`, so
        // `ImageExtractor` forces PSM 11, which skips Tesseract's full-page layout analysis).
        //
        // `extract_all_paragraphs` counts and WARNs on exactly which FFI call declined
        // (`skipped_no_para_info` for `TessPageIteratorParagraphInfo` returning 0, `skipped_no_bbox`
        // for `TessPageIteratorBoundingBox` returning 0 at RIL_PARA), giving a way to tell an FFI
        // refusal from a centroid/bbox matching bug:
        //   - `skipped_no_para_info > 0`                              -> Tesseract declined to describe
        //     paragraphs at all.
        //   - `skipped_no_bbox > 0`                                   -> a paragraph was described but had
        //     no RIL_PARA bbox.
        //   - both 0 AND `extracted_paragraphs > 0`                   -> the centroid-in-bbox match itself
        //     missed; the next diagnostic step would be logging the paragraph bbox and the word centroid.
        //
        // CI run 31366919399, cited by an earlier version of this comment, was CANCELLED on the
        // relevant legs and proves nothing; the confirmed arm-passes / macos-passes / x86_64-fails split
        // is from CI run 31316926033, a completed push to `main`.
        //
        // That run's log has zero occurrences of `skipped_no_para_info`/`skipped_no_bbox`: CI sets no
        // `RUST_LOG`, and (until now) this test installed no `tracing` subscriber, so the warn's fields
        // were never captured even though they exist. `captured_paragraph_skip_events` below fixes that:
        // `install_paragraph_skip_capture` registers a capture layer as the PROCESS-GLOBAL default, so a
        // captured event's fields are folded into this assertion's failure message and can be read
        // straight off the panic output without `RUST_LOG`.
        //
        // Global, not `with_default`, and that distinction is load-bearing: `perform_ocr` emits this warn
        // underneath `tokio::task::spawn_blocking` (see `ocr/tesseract_backend.rs`), i.e. on a
        // blocking-pool worker thread. `tracing`'s `with_default` override is thread-local, so a scoped
        // subscriber installed on the test thread would capture NOTHING here. See the type's doc comment
        // for why this does not repeat the interest-cache incident in project memory.
        //
        // Absence of paragraph metadata is therefore treated as a legitimate, architecture-dependent
        // outcome here rather than asserted away: keys must be present or absent together, and when
        // present, the expected value is still pinned. ~keep
        let has_is_crown = element.backend_metadata.contains_key("is_crown");
        let has_is_list_item = element.backend_metadata.contains_key("is_list_item");
        let has_justification = element.backend_metadata.contains_key("justification");
        assert_eq!(
            (has_is_crown, has_is_list_item, has_justification),
            (has_is_crown, has_is_crown, has_is_crown),
            "word {:?} paragraph metadata keys must be all-or-nothing (is_crown/is_list_item/justification \
             come from the same Option<&ParaInfo>). Full metadata: {:?}. Captured paragraph-skip diagnosis \
             events (skipped_no_para_info/skipped_no_bbox/extracted_paragraphs; empty means either no skips \
             occurred or the emitting `spawn_blocking` thread was not observed by this test's capture \
             layer): {:?}",
            element.text,
            element.backend_metadata,
            captured_paragraph_skip_events
        );
        if has_is_crown {
            assert_eq!(
                element.backend_metadata.get("is_crown"),
                Some(&serde_json::json!(false)),
                "word {:?} reported paragraph metadata (#191) but with an unexpected is_crown value. \
                 Full metadata: {:?}",
                element.text,
                element.backend_metadata
            );
        }
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
