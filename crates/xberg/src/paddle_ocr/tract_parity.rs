//! Cross-engine parity for classical PaddleOCR: ONNX Runtime vs the pure-Rust `tract` engine.
//!
//! Closes the verification half of issue #1354. The tract path ships so PaddleOCR runs on
//! targets where ORT cannot link (WASM, the Android x86_64 emulator); these tests are what
//! establishes that it produces the *same answers* as the ORT path it replaces there.
//!
//! # Running
//!
//! Both engines must be linked into one binary, so a single-engine feature set is not enough.
//! `paddle-ocr-ort` pulls in `dep:ort` but not an actual ONNX Runtime binary, so `ort-bundled`
//! is required too or every ORT session fails to create at run time:
//!
//! ```text
//! cargo test -p xberg --no-default-features \
//!     --features paddle-ocr-ort,paddle-ocr-tract,ort-bundled --lib paddle_ocr::tract_parity
//! ```
//!
//! # Non-vacuity
//!
//! Every test resolves its models through [`resolve_model`], which reads the local Hugging Face
//! cache and — only when `XBERG_REQUIRE_TRACT_PARITY` is set — downloads on a miss and panics on
//! failure. When a model is genuinely absent and parity is *not* required, the test prints a skip
//! note and returns, but not before asserting `!parity_required()`. CI sets the variable, so a
//! missing model or an unreadable page is a hard failure there rather than a green run that
//! compared nothing. The same guard covers the source page, which lives in the `test_documents`
//! submodule and is a Git LFS pointer until its content is fetched.
//!
//! # What is compared, and how strictly
//!
//! - **Recognition** and **classification** run on byte-identical inputs through unpinned plans
//!   on both engines, so their decoded text and predicted class are compared for exact equality;
//!   only the float confidences carry a tolerance.
//! - **Detection** runs the same tensor on both engines. tract cannot shape-infer DBNet with a
//!   symbolic H/W (the `Resize`-upsampled branch fails to unify against the FPN skip connection
//!   at `Concat`), so its plan is pinned — but pinned to the page's *own* resized extent, not to
//!   a canvas the page is padded into. That distinction is load-bearing and was the subject of
//!   the issue #1354 follow-up: DBNet's squeeze-and-excitation blocks average over the whole
//!   input, so padding shifts the probability map across the entire page rather than only at the
//!   seam. Boxes are still matched as a *set*, by intersection-over-union, as ADR 0027
//!   prescribes — and end-to-end results as word multisets rather than ordered lines, because a
//!   one-pixel box shift can reorder line grouping without changing a recognized character.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xberg_paddle_ocr::angle_net::AngleNet;
use xberg_paddle_ocr::base_net::BaseNet;
use xberg_paddle_ocr::crnn_net::CrnnNet;
use xberg_paddle_ocr::db_net::DbNet;
use xberg_paddle_ocr::ocr_utils::OcrUtils;
use xberg_paddle_ocr::scale_param::ScaleParam;
use xberg_paddle_ocr::{InferenceBackend, PaddleOcrEngine, TextBox};

use crate::inference::tract_backend::{PARITY_REPO, parity_required, resolve_model};
use crate::paddle_ocr::model_manager::HF_REPO_REVISION;

/// A detection/recognition model generation, named as the release names it.
struct ModelGeneration {
    label: &'static str,
    detection: &'static str,
    recognition: &'static str,
    recognition_dictionary: &'static str,
}

/// PP-OCRv5 lives under the `v2/` prefix in the model repository for historical reasons.
/// `en_mobile` is the small English-only recognizer — the cheapest v5 artifact that still
/// decodes Latin text, which keeps the download the tests force in CI down to a few megabytes.
const PP_OCR_V5: ModelGeneration = ModelGeneration {
    label: "PP-OCRv5",
    detection: "v2/det/mobile.onnx",
    recognition: "v2/rec/en_mobile/model.onnx",
    recognition_dictionary: "v2/rec/en_mobile/dict.txt",
};

/// PP-OCRv6, `tiny` tier for the same reason. Its recognizer was re-exported at the currently
/// pinned repository revision and now takes `[N, 3, 48, W]`.
const PP_OCR_V6: ModelGeneration = ModelGeneration {
    label: "PP-OCRv6",
    detection: "v6/det/tiny/model.onnx",
    recognition: "v6/rec/tiny/model.onnx",
    recognition_dictionary: "v6/rec/tiny/dict.txt",
};

const MODEL_GENERATIONS: &[ModelGeneration] = &[PP_OCR_V5, PP_OCR_V6];

/// Text-line orientation classifier. Shared across generations — there is no v6-specific
/// artifact, and the production `SharedModelPaths::cls_model` points every generation here.
const TEXTLINE_ORIENTATION_MODEL: &str = "v2/classifiers/PP-LCNet_x1_0_textline_ori.onnx";

/// A source page, relative to this crate's manifest directory.
struct ParityPage {
    label: &'static str,
    path: &'static str,
}

/// Both a trivial and a text-dense page.
///
/// The trivial one (800x200, a single rendered line) isolates a divergence to one region and
/// keeps failure output readable. It is not sufficient on its own: one box and two words would
/// let a systematic detection or grouping difference pass unnoticed, so the dense scan carries
/// the real weight — dozens of lines at varied sizes, including the small text where a shifted
/// box boundary actually changes which pixels reach the recognizer.
const SPARSE_PAGE: ParityPage = ParityPage {
    label: "hello-world",
    path: "../../test_documents/images/test_hello_world.png",
};

const DENSE_PAGE: ParityPage = ParityPage {
    label: "layout-parser-scan",
    path: "../../test_documents/images/layout_parser_ocr.jpg",
};

/// Every page. Used by the stages that are at full parity on all of them.
const PARITY_PAGES: &[ParityPage] = &[SPARSE_PAGE, DENSE_PAGE];

/// Long-side target handed to [`ScaleParam::get_scale_param`] for the detector-level tests.
const DETECTION_TARGET_SIDE: u32 = 640;
/// Long-side limit for the end-to-end engine tests, and the white margin
/// `PaddleOcrEngine::detect` adds on every side before scaling to it.
const E2E_MAX_SIDE_LEN: u32 = 640;
const E2E_PADDING: u32 = 10;

const BOX_SCORE_THRESHOLD: f32 = 0.5;
const BOX_THRESHOLD: f32 = 0.3;
const UN_CLIP_RATIO: f32 = 1.6;
const CLASSIFIER_THRESHOLD: f32 = 0.9;
const INFERENCE_THREADS: usize = 1;
const RECOGNITION_BATCH_SIZE: u32 = 1;

/// Tolerance on the classifier's softmax confidence and the recognizer's mean CTC score.
///
/// Both are single-precision reductions over an entire network; ORT and tract fuse and order
/// those reductions differently, so the last few mantissa bits are not expected to agree. The
/// bound is far tighter than any decision boundary either score feeds (the classifier threshold
/// is 0.9, the recognizer's angle-rollback threshold is a configurable ~0.6), so a divergence
/// large enough to change behavior cannot hide under it.
const CONFIDENCE_TOLERANCE: f32 = 1e-3;

/// Minimum intersection-over-union required between an ORT box and its tract counterpart.
///
/// Both engines now run the identical detection tensor, and every box on both parity pages and
/// both model generations matches at IoU 1.000 exactly — the bound absorbs no structural
/// difference and is deliberately not tuned down to the worst case that could still pass. What
/// legitimately can differ is a single pixel of the binarized mask: measured over the four
/// page/generation combinations here, the engines' raw probability maps differ by at most 5.0e-5
/// (mean 2.4e-7), which flipped 1 of 307200 pixels across the 0.3 binarization threshold on the
/// dense page and could flip a different one on another CPU.
///
/// 0.90 leaves roughly 3% linear slack, which covers that on every box actually emitted here. It
/// is *not* unconditionally safe: on the smallest box either engine produces (roughly 9x8 map
/// pixels) a one-pixel contour shift can cost about 0.2 IoU and would trip this bound. That is
/// the intended behavior — a box that small moving a whole pixel is worth a look, not a silently
/// widened tolerance. If this fires on another CPU, diagnose the shift; do not relax the bound to
/// make it pass. Region *counts* are asserted equal separately, which is what catches a merge.
const MIN_DETECTION_IOU: f32 = 0.90;

/// Load a parity page, or `None` when its content is unavailable and parity is not required.
fn parity_page(page: &ParityPage) -> Option<image::RgbImage> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(page.path);
    match image::open(&path) {
        Ok(image) => Some(image.to_rgb8()),
        Err(error) => {
            assert!(
                !parity_required(),
                "XBERG_REQUIRE_TRACT_PARITY is set but parity page {} could not be read: {} ({error}) \
                 — check out the test_documents submodule and fetch its Git LFS content",
                page.label,
                path.display()
            );
            eprintln!("skip: parity page unavailable at {} ({error})", path.display());
            None
        }
    }
}

/// Resolve a parity model **at the revision production pins**, or `None` when it is absent and
/// parity is not required.
///
/// Deliberately not [`resolve_model`] alone. That helper scans the whole Hugging Face cache for a
/// path suffix and keeps the last match, which is fine for artifacts that exist in exactly one
/// snapshot but silently picks an arbitrary one for a repository with several — and this
/// repository has several. The PP-OCRv6 recognizers were re-exported at
/// [`HF_REPO_REVISION`] with a different input signature, so an unpinned lookup can hand the test
/// a superseded model that production never loads. The pinned snapshot is checked first; the
/// shared helper is still the fallback so the download-and-fail-loudly behavior under
/// `XBERG_REQUIRE_TRACT_PARITY` is inherited rather than reimplemented.
fn parity_model(suffix: &str) -> Option<PathBuf> {
    if let Some(pinned) = pinned_revision_model(suffix) {
        return Some(pinned);
    }
    if parity_required() {
        let downloaded = crate::model_download::hf_download_revision(PARITY_REPO, suffix, HF_REPO_REVISION)
            .unwrap_or_else(|error| {
                panic!(
                    "XBERG_REQUIRE_TRACT_PARITY is set but downloading {suffix} at {HF_REPO_REVISION} failed: {error}"
                )
            });
        return Some(downloaded);
    }
    let resolved = resolve_model(PARITY_REPO, suffix);
    if resolved.is_none() {
        eprintln!("skip: {suffix} not in the Hugging Face cache at revision {HF_REPO_REVISION}");
    }
    resolved
}

/// The artifact inside the pinned snapshot directory of the standard Hub cache, when present.
fn pinned_revision_model(suffix: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = Path::new(&home)
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", PARITY_REPO.replace('/', "--")))
        .join("snapshots")
        .join(HF_REPO_REVISION)
        .join(suffix);
    path.exists().then_some(path)
}

fn as_str(path: &Path) -> &str {
    path.to_str().expect("model paths in the HF cache are valid UTF-8")
}

fn load_detector(backend: InferenceBackend, model: &Path) -> DbNet {
    let mut detector = DbNet::new();
    detector
        .init_model_on(backend, as_str(model), INFERENCE_THREADS)
        .unwrap_or_else(|error| panic!("{backend:?} must load the detection model {model:?}: {error}"));
    detector
}

fn load_recognizer(backend: InferenceBackend, model: &Path, dictionary: &Path) -> CrnnNet {
    let mut recognizer = CrnnNet::new();
    recognizer
        .init_model_dict_file_on(backend, as_str(model), INFERENCE_THREADS, as_str(dictionary))
        .unwrap_or_else(|error| panic!("{backend:?} must load the recognition model {model:?}: {error}"));
    recognizer
}

fn load_classifier(backend: InferenceBackend, model: &Path) -> AngleNet {
    let mut classifier = AngleNet::new();
    classifier
        .init_model_on(backend, as_str(model), INFERENCE_THREADS)
        .unwrap_or_else(|error| panic!("{backend:?} must load the orientation classifier {model:?}: {error}"));
    classifier
}

fn detect(detector: &DbNet, page: &image::RgbImage) -> Vec<TextBox> {
    let scale = ScaleParam::get_scale_param(page, DETECTION_TARGET_SIDE);
    detector
        .get_text_boxes(page, &scale, BOX_SCORE_THRESHOLD, BOX_THRESHOLD, UN_CLIP_RATIO)
        .expect("detection must run")
}

/// Axis-aligned bounds of a (possibly rotated) detection quad: `(left, top, right, bottom)`.
fn bounds(text_box: &TextBox) -> (f32, f32, f32, f32) {
    let mut left = f32::MAX;
    let mut top = f32::MAX;
    let mut right = f32::MIN;
    let mut bottom = f32::MIN;
    for point in &text_box.points {
        left = left.min(point.x as f32);
        top = top.min(point.y as f32);
        right = right.max(point.x as f32);
        bottom = bottom.max(point.y as f32);
    }
    (left, top, right, bottom)
}

/// Intersection-over-union of two detections' axis-aligned bounds.
///
/// Compared on bounds rather than the raw quads because the quads are contour output whose
/// vertex *ordering* is an implementation detail of the marching step; the occupied region is
/// the part that carries meaning for "did both engines find the same text".
fn intersection_over_union(left_box: &TextBox, right_box: &TextBox) -> f32 {
    let (a_left, a_top, a_right, a_bottom) = bounds(left_box);
    let (b_left, b_top, b_right, b_bottom) = bounds(right_box);

    let overlap_width = (a_right.min(b_right) - a_left.max(b_left)).max(0.0);
    let overlap_height = (a_bottom.min(b_bottom) - a_top.max(b_top)).max(0.0);
    let intersection = overlap_width * overlap_height;

    let a_area = (a_right - a_left) * (a_bottom - a_top);
    let b_area = (b_right - b_left) * (b_bottom - b_top);
    let union = a_area + b_area - intersection;
    if union <= 0.0 { 0.0 } else { intersection / union }
}

/// For every box in `expected`, the IoU of its best counterpart in `actual`.
fn best_overlaps(expected: &[TextBox], actual: &[TextBox]) -> Vec<f32> {
    expected
        .iter()
        .map(|candidate| {
            actual
                .iter()
                .map(|other| intersection_over_union(candidate, other))
                .fold(0.0_f32, f32::max)
        })
        .collect()
}

/// Whitespace-separated words, sorted — a multiset, so line grouping and reading order drop out
/// while a dropped, gained, or misspelled word still shows up.
fn word_multiset<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut words: Vec<String> = lines
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect();
    words.sort_unstable();
    words
}

/// Detection boxes from a single engine, reused as the recognition and classification inputs so
/// those two comparisons see byte-identical crops and are not contaminated by detection jitter.
fn shared_crops(page: &image::RgbImage, detection_model: &Path) -> Vec<image::RgbImage> {
    let detector = load_detector(InferenceBackend::Ort, detection_model);
    let boxes = detect(&detector, page);
    OcrUtils::get_part_images(page, &boxes)
}

/// Recognition runs an unpinned plan on both engines, so the same crops must decode to exactly
/// the same characters. Anything less is a real divergence, not a rounding artifact.
#[test]
fn should_decode_identical_text_on_both_engines_for_every_generation() {
    let mut compared = 0;
    for page_source in PARITY_PAGES {
        let Some(page) = parity_page(page_source) else { continue };
        for generation in MODEL_GENERATIONS {
            let scope = format!("{}/{}", page_source.label, generation.label);
            let (Some(detection), Some(recognition), Some(dictionary)) = (
                parity_model(generation.detection),
                parity_model(generation.recognition),
                parity_model(generation.recognition_dictionary),
            ) else {
                continue;
            };

            let crops = shared_crops(&page, &detection);
            assert!(
                !crops.is_empty(),
                "{scope}: detection produced no crops, so recognition parity would compare nothing"
            );

            let ort = load_recognizer(InferenceBackend::Ort, &recognition, &dictionary);
            let tract = load_recognizer(InferenceBackend::Tract, &recognition, &dictionary);

            let empty = HashMap::new();
            let ort_lines = ort
                .get_text_lines(&crops, &empty, 0.0, RECOGNITION_BATCH_SIZE)
                .expect("ORT recognition must run");
            let tract_lines = tract
                .get_text_lines(&crops, &empty, 0.0, RECOGNITION_BATCH_SIZE)
                .expect("tract recognition must run");

            let differing = ort_lines
                .iter()
                .zip(&tract_lines)
                .filter(|(ort_line, tract_line)| ort_line.text != tract_line.text)
                .count();
            eprintln!(
                "{scope}: {} crops, {differing} decoded differently; first ORT line {:?}",
                crops.len(),
                ort_lines.first().map(|line| &line.text)
            );

            assert_eq!(
                ort_lines.len(),
                crops.len(),
                "{scope}: recognition must return one line per crop"
            );
            assert_eq!(
                tract_lines.len(),
                ort_lines.len(),
                "{scope}: engines returned different line counts"
            );
            assert!(
                ort_lines.iter().any(|line| !line.text.trim().is_empty()),
                "{scope}: every line decoded empty, so the comparison proves nothing"
            );

            for (index, (ort_line, tract_line)) in ort_lines.iter().zip(&tract_lines).enumerate() {
                assert_eq!(
                    ort_line.text, tract_line.text,
                    "{scope}: crop {index} decoded differently — ORT {:?} vs tract {:?}",
                    ort_line.text, tract_line.text
                );
                let score_delta = (ort_line.text_score - tract_line.text_score).abs();
                assert!(
                    score_delta < CONFIDENCE_TOLERANCE,
                    "{scope}: crop {index} score diverged by {score_delta} (ORT {} vs tract {})",
                    ort_line.text_score,
                    tract_line.text_score
                );
            }
            compared += 1;
        }
    }

    assert!(
        compared > 0 || !parity_required(),
        "XBERG_REQUIRE_TRACT_PARITY is set but no recognition models were compared"
    );
}

/// The text-line orientation classifier is a fixed-shape PP-LCNet loaded unpinned on both
/// engines: the predicted class must match exactly, and only the softmax confidence is allowed
/// to drift within float tolerance.
#[test]
fn should_predict_the_same_textline_orientation_class_on_both_engines() {
    let Some(classifier_model) = parity_model(TEXTLINE_ORIENTATION_MODEL) else {
        assert!(
            !parity_required(),
            "XBERG_REQUIRE_TRACT_PARITY is set but the orientation classifier was not compared"
        );
        return;
    };
    let Some(detection) = parity_model(PP_OCR_V5.detection) else {
        assert!(
            !parity_required(),
            "XBERG_REQUIRE_TRACT_PARITY is set but the orientation classifier was not compared"
        );
        return;
    };

    let ort = load_classifier(InferenceBackend::Ort, &classifier_model);
    let tract = load_classifier(InferenceBackend::Tract, &classifier_model);

    let mut compared = 0;
    for page_source in PARITY_PAGES {
        let Some(page) = parity_page(page_source) else { continue };
        let scope = page_source.label;

        let crops = shared_crops(&page, &detection);
        assert!(
            !crops.is_empty(),
            "{scope}: classification parity needs at least one crop"
        );

        // `most_angle = false`: the majority-vote pass would overwrite every per-crop prediction
        // with one aggregate index and hide a genuine per-crop disagreement.
        let ort_angles = ort
            .get_angles(&crops, true, false, CLASSIFIER_THRESHOLD)
            .expect("ORT classification must run");
        let tract_angles = tract
            .get_angles(&crops, true, false, CLASSIFIER_THRESHOLD)
            .expect("tract classification must run");

        let max_score_delta = ort_angles
            .iter()
            .zip(&tract_angles)
            .map(|(ort_angle, tract_angle)| (ort_angle.score - tract_angle.score).abs())
            .fold(0.0_f32, f32::max);
        eprintln!(
            "{scope}: {} crops classified, max |Δconfidence| {max_score_delta}",
            crops.len()
        );

        assert_eq!(
            ort_angles.len(),
            crops.len(),
            "{scope}: classification must return one angle per crop"
        );
        assert_eq!(
            tract_angles.len(),
            ort_angles.len(),
            "{scope}: engines returned different angle counts"
        );

        for (index, (ort_angle, tract_angle)) in ort_angles.iter().zip(&tract_angles).enumerate() {
            assert_eq!(
                ort_angle.index, tract_angle.index,
                "{scope}: crop {index} predicted orientation class diverged (ORT {} vs tract {})",
                ort_angle.index, tract_angle.index
            );
            let score_delta = (ort_angle.score - tract_angle.score).abs();
            assert!(
                score_delta < CONFIDENCE_TOLERANCE,
                "{scope}: crop {index} orientation confidence diverged by {score_delta} (ORT {} vs tract {})",
                ort_angle.score,
                tract_angle.score
            );
        }
        compared += 1;
    }

    assert!(
        compared > 0 || !parity_required(),
        "XBERG_REQUIRE_TRACT_PARITY is set but no page was classified"
    );
}

/// Detection on every parity page, including the text-dense scan that reproduced the issue
/// #1354 follow-up defect. Boxes are compared as a set, by IoU, never coordinate-for-coordinate.
#[test]
fn should_detect_the_same_regions_on_both_engines() {
    compare_detection_over(PARITY_PAGES);
}

fn compare_detection_over(pages: &[ParityPage]) {
    let mut compared = 0;
    for page_source in pages {
        let Some(page) = parity_page(page_source) else { continue };
        for generation in MODEL_GENERATIONS {
            let scope = format!("{}/{}", page_source.label, generation.label);
            let Some(model) = parity_model(generation.detection) else {
                continue;
            };

            let ort_boxes = detect(&load_detector(InferenceBackend::Ort, &model), &page);
            let tract_boxes = detect(&load_detector(InferenceBackend::Tract, &model), &page);

            let forward = best_overlaps(&ort_boxes, &tract_boxes);
            let reverse = best_overlaps(&tract_boxes, &ort_boxes);
            let worst_forward = forward.iter().copied().fold(f32::MAX, f32::min);
            let worst_reverse = reverse.iter().copied().fold(f32::MAX, f32::min);
            eprintln!(
                "{scope}: ORT {} boxes, tract {} boxes; worst IoU ORT->tract {worst_forward}, tract->ORT {worst_reverse}",
                ort_boxes.len(),
                tract_boxes.len()
            );

            assert!(
                !ort_boxes.is_empty(),
                "{scope}: ORT detected nothing, so there is nothing to compare"
            );
            assert_eq!(
                tract_boxes.len(),
                ort_boxes.len(),
                "{scope}: engines disagree on how many regions exist (ORT {} vs tract {}) — \
                 a split or merged region",
                ort_boxes.len(),
                tract_boxes.len()
            );

            // Checked in both directions: one-way matching would pass if tract collapsed several
            // ORT regions onto one box that happens to overlap each of them well.
            for (index, overlap) in forward.iter().enumerate() {
                assert!(
                    *overlap >= MIN_DETECTION_IOU,
                    "{scope}: ORT region {index} has no tract counterpart above IoU {MIN_DETECTION_IOU} (best {overlap})"
                );
            }
            for (index, overlap) in reverse.iter().enumerate() {
                assert!(
                    *overlap >= MIN_DETECTION_IOU,
                    "{scope}: tract region {index} has no ORT counterpart above IoU {MIN_DETECTION_IOU} (best {overlap})"
                );
            }
            compared += 1;
        }
    }

    assert!(
        compared > 0 || !parity_required(),
        "XBERG_REQUIRE_TRACT_PARITY is set but no detection models were compared"
    );
}

/// The whole pipeline, through the same `PaddleOcrEngine` entry point production uses.
///
/// Compared as a word multiset: detection jitter can move a box boundary enough to change how
/// lines are grouped or ordered without changing a single recognized character, and a test that
/// demanded identical ordered lines would fail on that cosmetic difference while telling us
/// nothing about whether tract reads the page correctly.
#[test]
fn should_recognize_the_same_word_multiset_end_to_end_on_both_engines() {
    compare_end_to_end_over(PARITY_PAGES);
}

fn compare_end_to_end_over(pages: &[ParityPage]) {
    let mut compared = 0;
    for page_source in pages {
        let Some(page) = parity_page(page_source) else { continue };
        for generation in MODEL_GENERATIONS {
            let scope = format!("{}/{}", page_source.label, generation.label);
            let (Some(detection), Some(recognition), Some(dictionary)) = (
                parity_model(generation.detection),
                parity_model(generation.recognition),
                parity_model(generation.recognition_dictionary),
            ) else {
                continue;
            };
            let Some(classifier) = parity_model(TEXTLINE_ORIENTATION_MODEL) else {
                continue;
            };

            let build = |backend: InferenceBackend| {
                let mut engine = PaddleOcrEngine::new();
                engine
                    .init_models_with_dict_on(
                        backend,
                        as_str(&detection),
                        as_str(&classifier),
                        as_str(&recognition),
                        as_str(&dictionary),
                        INFERENCE_THREADS,
                    )
                    .unwrap_or_else(|error| panic!("{scope}: {backend:?} engine init: {error}"));
                engine
            };
            let run = |engine: &PaddleOcrEngine, backend: InferenceBackend| {
                engine
                    .detect(
                        &page,
                        E2E_PADDING,
                        E2E_MAX_SIDE_LEN,
                        BOX_SCORE_THRESHOLD,
                        BOX_THRESHOLD,
                        UN_CLIP_RATIO,
                        true,
                        false,
                    )
                    .unwrap_or_else(|error| panic!("{scope}: {backend:?} end-to-end detect: {error}"))
            };

            let ort_result = run(&build(InferenceBackend::Ort), InferenceBackend::Ort);
            let tract_result = run(&build(InferenceBackend::Tract), InferenceBackend::Tract);

            let ort_words = word_multiset(ort_result.text_blocks.iter().map(|block| block.text.as_str()));
            let tract_words = word_multiset(tract_result.text_blocks.iter().map(|block| block.text.as_str()));
            eprintln!(
                "{scope}: end-to-end ORT {} blocks / {} words; tract {} blocks / {} words",
                ort_result.text_blocks.len(),
                ort_words.len(),
                tract_result.text_blocks.len(),
                tract_words.len()
            );

            assert!(
                !ort_words.is_empty(),
                "{scope}: ORT recognized no words end to end, so the multiset comparison is empty"
            );
            assert_eq!(
                ort_words, tract_words,
                "{scope}: end-to-end word multisets diverged between engines"
            );
            compared += 1;
        }
    }

    assert!(
        compared > 0 || !parity_required(),
        "XBERG_REQUIRE_TRACT_PARITY is set but no end-to-end pipeline was compared"
    );
}

/// Guards the premise the whole module rests on. `xberg_paddle_ocr`'s default backend resolves at
/// compile time and prefers ORT whenever the `ort` feature is on, so in this dual-engine build
/// every `init_model*` call that does *not* name an engine loads ORT. If the explicit selection
/// regressed to the default, all four parity tests above would compare ORT against ORT and pass
/// while proving nothing — the vacuous-pass failure mode this suite exists to avoid.
#[test]
fn should_load_distinct_engines_when_the_backend_is_named_explicitly() {
    let Some(model) = parity_model(TEXTLINE_ORIENTATION_MODEL) else {
        assert!(
            !parity_required(),
            "XBERG_REQUIRE_TRACT_PARITY is set but engine selection was not verified"
        );
        return;
    };

    let mut ort = AngleNet::new();
    ort.init_model_on(InferenceBackend::Ort, as_str(&model), INFERENCE_THREADS)
        .expect("explicit ORT load must succeed");
    let mut tract = AngleNet::new();
    tract
        .init_model_on(InferenceBackend::Tract, as_str(&model), INFERENCE_THREADS)
        .expect("explicit tract load must succeed");

    // `Debug` on the nets reports the loaded backend's `name()`, which is the only externally
    // visible proof of which engine a net actually holds. ~keep
    let ort_debug = format!("{ort:?}");
    let tract_debug = format!("{tract:?}");
    eprintln!("engine selection: ORT {ort_debug}, tract {tract_debug}");
    assert!(
        ort_debug.contains("backend: Some(\"ort\")"),
        "a net asked for ORT did not load ORT: {ort_debug}"
    );
    assert!(
        tract_debug.contains("backend: Some(\"tract\")"),
        "a net asked for tract loaded a different engine — explicit backend selection is not taking \
         effect, so every parity test in this module is comparing an engine against itself: {tract_debug}"
    );
}
