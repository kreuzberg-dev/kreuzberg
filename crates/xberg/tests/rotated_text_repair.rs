//! GH#1358 — sideways (90/180/270-degree text-matrix) runs must assemble in their own
//! reading order, on the DEFAULT extraction config.
//!
//! The reporter's symptom is a rotated spec table coming out word-reversed and glued
//! (`the meet only need oil Engine`). The repair for that lives in
//! `extractors::pdf::reading_order` and — this is the part these tests pin — it must run
//! *without* `pdf_options.reading_order`, *without* layout hints, *without* ONNX
//! inference and *without* the `layout-detection` feature. Every test below therefore
//! builds its config as `ExtractionConfig { use_cache: false, ..Default::default() }` and
//! never sets `pdf_options`. If someone re-gates the repair behind a flag or a feature,
//! these go red.
//!
//! `use_cache` is forced off everywhere: the result cache is keyed on content plus
//! config, so a cached entry written by a pre-fix run of the same bytes would answer
//! these tests from the broken code.
//!
//! # Why synthetic PDFs
//!
//! The repair is only observable when a rotated run's *content-stream* order differs
//! from its geometric reading order — that is exactly the reordering it performs. Every
//! rotated run in the bundled corpus was checked (`mutool clean -d` over all 228 PDFs in
//! `test_documents/pdf/`): each one is a single `Tj` operator, so no corpus fixture can
//! discriminate a fixed build from a broken one. `ocr_test_rotated_{90,180,270}.pdf` are
//! pure 4960x7016 scans with a zero-length text layer and cannot exercise this path at
//! all without OCR.
//!
//! So the discriminating cases are three tiny hand-built PDFs whose three words are
//! emitted in *reverse* of the run's reading order. Their expected output was confirmed
//! against two independent oracles before being pinned here: poppler (`pdftotext`) and
//! MuPDF (`mutool draw -F stext`) both read all three as `ENGINE OIL LEVEL`, while
//! `pdftotext -raw` (content-stream order) reads `LEVEL OIL ENGINE`. A build that skips
//! the repair produces the latter.
//!
//! # Why the safety tests matter more than the rotation tests
//!
//! `pdf_markdown_regression.rs` runs on `..Default::default()` and carries ~173 hand
//! calibrated F1 thresholds. If this work perturbs *unrotated* text by even a word, that
//! whole gate is silently invalidated. `unrotated_*` below is the guard: an upright page
//! whose words are already in reading order must come back with those words untouched
//! and unglued, and a real 9-page unrotated corpus document must keep its tokens in
//! ground-truth order.

#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
#![cfg(feature = "pdf")]

use std::path::{Path, PathBuf};

use xberg::core::config::ExtractInput;
use xberg::{ExtractionConfig, extract};

// ═══════════════════════════════════════════════════════════════════
// Synthetic PDF construction
// ═══════════════════════════════════════════════════════════════════

/// Text matrix for an unrotated run: advances toward +x.
const MATRIX_UPRIGHT: &str = "1 0 0 1";
/// Text matrix for a 90-degree run: advances toward +y in page space.
const MATRIX_ROTATED_90: &str = "0 1 -1 0";
/// Text matrix for a 180-degree run: advances toward -x in page space.
const MATRIX_ROTATED_180: &str = "-1 0 0 -1";
/// Text matrix for a 270-degree run: advances toward -y in page space.
const MATRIX_ROTATED_270: &str = "0 -1 1 0";

/// The three words every synthetic fixture carries, in the order a human reads them.
const READING_ORDER_WORDS: [&str; 3] = ["ENGINE", "OIL", "LEVEL"];

/// What the broken (content-stream-order) assembly produces for the rotated fixtures.
const SCRAMBLED_PHRASE: &str = "LEVEL OIL ENGINE";

/// What a correct assembly produces. Confirmed independently by poppler and MuPDF.
const REPAIRED_PHRASE: &str = "ENGINE OIL LEVEL";

/// The same three words as `pdf_oxide` actually emits them on an **upright** run: glued.
///
/// `pdf_oxide` drops inter-word spaces on the upright path — it merges adjacent upright
/// spans and concatenates their text verbatim, inserting nothing between them. Still
/// present at 0.3.77 (upstream #847; it affects 44 of the 157 corpus documents). Probed
/// against this exact fixture: `1 0 0 1` yields a single span `"LEVELOILENGINE"`.
///
/// The *rotated* path is the one that inserts gap-derived spaces, which is why a repaired
/// rotated run below reads better-separated than ordinary upright body text — an artefact
/// of the upstream asymmetry, not of this repair.
///
/// Pinned here as the pre-change baseline. These tests exist to prove the rotation repair
/// does not disturb upright text, so the honest reference is what upright extraction
/// really produces, not an idealised string it has never produced. When upstream #847
/// lands, this constant goes red — which is the signal we want.
const UPRIGHT_PHRASE_AS_PDF_OXIDE_EMITS_IT: &str = "ENGINEOILLEVEL";

/// One `Tm` + `Tj` pair: place `word` at (`x`, `y`) under the given text matrix.
fn show_text(matrix: &str, x: u32, y: u32, word: &str) -> String {
    format!("{matrix} {x} {y} Tm\n({word}) Tj\n")
}

/// Wrap a sequence of text-placement operators into a single-page PDF.
///
/// Deliberately uncompressed with a classic cross-reference table so the bytes stay
/// readable in a failure dump and no filter code sits between the test and the parser.
/// The layout is fixed: 1 catalog, 2 pages, 3 page, 4 contents, 5 font.
fn single_page_pdf(operators: &str) -> Vec<u8> {
    let content = format!("BT\n/F1 24 Tf\n{operators}ET\n");
    let objects: [String; 5] = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];

    let mut bytes: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }

    let xref_offset = bytes.len();
    let size = objects.len() + 1;
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    bytes
}

/// A page holding one rotated run whose words are emitted in reverse reading order.
///
/// All three words share a cross-axis coordinate, so a rotated-frame line clusterer sees
/// one line; they differ only along the run's own advance axis, where sorting yields
/// [`READING_ORDER_WORDS`]. Gaps are ~10-16pt at 24pt Helvetica — ordinary word gaps,
/// not column gaps.
fn rotated_run_pdf(rotation_degrees: u32) -> Vec<u8> {
    // (matrix, positions listed in *emission* order, i.e. LEVEL, OIL, ENGINE)
    let (matrix, positions): (&str, [(u32, u32); 3]) = match rotation_degrees {
        90 => (MATRIX_ROTATED_90, [(300, 350), (300, 300), (300, 200)]),
        180 => (MATRIX_ROTATED_180, [(350, 400), (400, 400), (500, 400)]),
        270 => (MATRIX_ROTATED_270, [(300, 450), (300, 500), (300, 600)]),
        other => panic!("rotated_run_pdf only builds 90/180/270-degree runs, got {other}"),
    };

    let emission_order = ["LEVEL", "OIL", "ENGINE"];
    let mut operators = String::new();
    for (word, (x, y)) in emission_order.iter().zip(positions) {
        operators.push_str(&show_text(matrix, x, y, word));
    }
    single_page_pdf(&operators)
}

/// An upright page whose words are emitted in reading order — the control.
///
/// Content-stream order and geometric order agree here on purpose, so the expected
/// output is unambiguous under either assembly strategy and the assertion can be exact.
fn upright_run_pdf() -> Vec<u8> {
    let mut operators = String::new();
    for (word, x) in READING_ORDER_WORDS.iter().zip([100_u32, 220, 300]) {
        operators.push_str(&show_text(MATRIX_UPRIGHT, x, 700, word));
    }
    single_page_pdf(&operators)
}

/// A page carrying an upright run *and* a rotated run, to pin the run boundary.
///
/// The rotated run must be reassembled; the upright run beside it must not be dragged
/// into the rotated frame.
fn mixed_upright_and_rotated_pdf() -> Vec<u8> {
    let mut operators = String::new();
    for (word, x) in ["ALPHA", "BETA", "GAMMA"].iter().zip([60_u32, 160, 250]) {
        operators.push_str(&show_text(MATRIX_UPRIGHT, x, 700, word));
    }
    for (word, y) in ["LEVEL", "OIL", "ENGINE"].iter().zip([350_u32, 300, 200]) {
        operators.push_str(&show_text(MATRIX_ROTATED_90, 300, y, word));
    }
    single_page_pdf(&operators)
}

// ═══════════════════════════════════════════════════════════════════
// Shared assertion helpers
// ═══════════════════════════════════════════════════════════════════

/// The config under test: stock defaults, cache off, `pdf_options` untouched.
fn default_config_without_cache() -> ExtractionConfig {
    ExtractionConfig {
        // See the module docs: a cache hit would answer from a pre-fix run.
        use_cache: false,
        ..Default::default()
    }
}

/// Extract synthetic PDF bytes through the public API and return the document text.
async fn extract_pdf_text(bytes: Vec<u8>, description: &str) -> String {
    let input = ExtractInput::from_bytes(bytes, "application/pdf", None);
    let result = extract(input, &default_config_without_cache())
        .await
        .unwrap_or_else(|error| panic!("extraction of the {description} fixture must succeed: {error}"));
    result
        .results
        .first()
        .unwrap_or_else(|| panic!("one input must yield exactly one result for the {description} fixture"))
        .content
        .clone()
}

/// Collapse every whitespace run to a single space so assertions pin word order and word
/// separation without pinning line breaking, which is not what GH#1358 is about.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Assert `words` appear in `text` in exactly this order, each exactly once.
///
/// Order alone would still pass if a word were duplicated by a double-assembly bug, so
/// the occurrence count is pinned too.
fn assert_words_in_order(text: &str, words: &[&str], context: &str) {
    let mut previous_index: Option<usize> = None;
    for word in words {
        let occurrences = text.matches(word).count();
        assert_eq!(
            occurrences, 1,
            "{context}: expected {word:?} exactly once, found it {occurrences} times in {text:?}"
        );
        let index = text
            .find(word)
            .unwrap_or_else(|| panic!("{context}: {word:?} is missing entirely from the extracted text {text:?}"));
        if let Some(previous) = previous_index {
            assert!(
                previous < index,
                "{context}: {word:?} must follow the previous word of the run but appeared before it \
                 (byte {index} vs {previous}) in {text:?}"
            );
        }
        previous_index = Some(index);
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xberg has a grandparent")
        .to_path_buf()
}

/// Corpus fixtures these tests read. Bucket-fetched, absent from a bare checkout.
const CORPUS_FIXTURES: [&str; 3] = [
    "test_documents/pdf/tatr.pdf",
    "test_documents/pdf/2026-13845.pdf",
    "test_documents/pdf/pdfa_018.pdf",
];

/// Resolve a corpus fixture, or `None` with an explanatory skip note.
fn corpus_fixture(relative_path: &str) -> Option<PathBuf> {
    let path = repo_root().join(relative_path);
    if path.is_file() {
        return Some(path);
    }
    eprintln!(
        "SKIP: {} is absent — run `python3 test_documents/scripts/fetch_corpus.py`. \
         `corpus_fixtures_for_rotation_repair_must_be_present` covers the fully-absent case.",
        path.display()
    );
    None
}

/// Extract a corpus PDF from disk through the public API and return its text.
async fn extract_corpus_text(path: &Path) -> String {
    let input = ExtractInput::from_uri(path.to_string_lossy());
    let result = extract(input, &default_config_without_cache())
        .await
        .unwrap_or_else(|error| panic!("extraction of {} must succeed: {error}", path.display()));
    result
        .results
        .first()
        .unwrap_or_else(|| panic!("one input must yield exactly one result for {}", path.display()))
        .content
        .clone()
}

// ═══════════════════════════════════════════════════════════════════
// The rotation repair, on the default config
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn rotated_90_run_is_assembled_in_reading_order_on_the_default_config() {
    let text = normalize_whitespace(&extract_pdf_text(rotated_run_pdf(90), "90-degree rotated").await);

    assert!(
        text.contains(REPAIRED_PHRASE),
        "a 90-degree rotated run must read {REPAIRED_PHRASE:?} along its own advance axis \
         (poppler and MuPDF both read it that way); extracted text was {text:?}"
    );
    assert!(
        !text.contains(SCRAMBLED_PHRASE),
        "a 90-degree rotated run must not come back in content-stream order {SCRAMBLED_PHRASE:?} \
         — that is the GH#1358 word-reversal; extracted text was {text:?}"
    );
    assert_words_in_order(&text, &READING_ORDER_WORDS, "90-degree rotated run");
}

/// Blocked upstream in `pdf_oxide` (yfedoseev/pdf_oxide#1029), which never reports a
/// 180-degree run as rotated.
///
/// `pdf_oxide-0.3.77/src/extractors/text.rs:1931`, `snap_run_rotation`, opens with a
/// "pure horizontal fast path" that returns `0.0` whenever both off-diagonal terms of the
/// composed matrix are ~0:
///
/// ```text
/// if b.abs() < 1e-4 && c.abs() < 1e-4 { return 0.0; }
/// ```
///
/// A 180-degree flip is `-1 0 0 -1`: `b == c == 0`, so it takes that path and never
/// reaches the `atan2(b, a)` two lines below, which would have yielded exactly `180.0`.
/// The test only inspects the off-diagonal terms; it ignores the signs of `a` and `d`,
/// which are the only thing distinguishing a flip from upright text.
///
/// Both symptoms follow from that one line. Probed directly against 0.3.77:
///
/// ```text
/// rot90    0 1 -1 0: 3 spans  ("LEVEL" rot=90, "OIL" rot=90, "ENGINE" rot=90)
/// rot270   0 -1 1 0: 3 spans  ("ENGINE" rot=-90, "OIL" rot=-90, "LEVEL" rot=-90)
/// rot180  -1 0 0 -1: 1 span   ("LEVELOILENGINE" rot=0)
/// ```
///
/// With rotation reported as 0 the upright merger folds the whole run into a single span
/// and concatenates the text, so the word boundaries are destroyed before xberg sees the
/// page. No detector on this side can recover them — there is nothing left to reorder.
/// 90 and 270 arrive as three separate spans carrying a real angle, which is why they are
/// repairable and this is not.
///
/// Kept as a live assertion rather than deleted: when `pdf_oxide` fixes the fast path this
/// test starts passing, and `cargo test -- --ignored` is where we will see it.
#[ignore = "blocked on pdf_oxide: snap_run_rotation's horizontal fast path reports -1 0 0 -1 as \
            rotation 0, so the run is merged into one span and the words are unrecoverable"]
#[tokio::test]
async fn rotated_180_run_is_assembled_in_reading_order_on_the_default_config() {
    let text = normalize_whitespace(&extract_pdf_text(rotated_run_pdf(180), "180-degree rotated").await);

    assert!(
        text.contains(REPAIRED_PHRASE),
        "a 180-degree rotated run must read {REPAIRED_PHRASE:?} along its own (-x) advance axis; \
         extracted text was {text:?}"
    );
    assert!(
        !text.contains(SCRAMBLED_PHRASE),
        "a 180-degree rotated run must not come back in content-stream order {SCRAMBLED_PHRASE:?}; \
         extracted text was {text:?}"
    );
    assert_words_in_order(&text, &READING_ORDER_WORDS, "180-degree rotated run");
}

#[tokio::test]
async fn rotated_270_run_is_assembled_in_reading_order_on_the_default_config() {
    let text = normalize_whitespace(&extract_pdf_text(rotated_run_pdf(270), "270-degree rotated").await);

    assert!(
        text.contains(REPAIRED_PHRASE),
        "a 270-degree rotated run must read {REPAIRED_PHRASE:?} along its own (-y) advance axis; \
         extracted text was {text:?}"
    );
    assert!(
        !text.contains(SCRAMBLED_PHRASE),
        "a 270-degree rotated run must not come back in content-stream order {SCRAMBLED_PHRASE:?}; \
         extracted text was {text:?}"
    );
    assert_words_in_order(&text, &READING_ORDER_WORDS, "270-degree rotated run");
}

/// The name is the specification: no `pdf_options` object exists at all in this config.
///
/// `pdf_options.reading_order` defaults to false and the layout-guided reordering path is
/// `#[cfg(feature = "layout-detection")]`. Neither may be a prerequisite for repairing a
/// rotated run, because the reporter hits this with a stock build and a stock config.
#[tokio::test]
async fn rotation_repair_runs_with_pdf_options_left_as_none() {
    let config = default_config_without_cache();
    assert!(
        config.pdf_options.is_none(),
        "this test is only meaningful while `pdf_options` defaults to None; it is {:?}",
        config.pdf_options
    );

    let input = ExtractInput::from_bytes(rotated_run_pdf(90), "application/pdf", None);
    let result = extract(input, &config)
        .await
        .expect("extraction with pdf_options = None must succeed");
    let text = normalize_whitespace(&result.results.first().expect("one result").content);

    assert!(
        text.contains(REPAIRED_PHRASE),
        "the rotated-run repair must not require `pdf_options.reading_order` or the \
         `layout-detection` feature; with pdf_options = None the text was {text:?}"
    );
}

/// A rotated run must not swallow the upright text sitting next to it.
///
/// Only the rotated words are reordered; the upright run keeps its own order, and no
/// word from either run may cross into the other's frame.
#[tokio::test]
async fn upright_run_beside_a_rotated_run_keeps_its_own_order() {
    let raw = extract_pdf_text(mixed_upright_and_rotated_pdf(), "mixed upright + rotated").await;
    let text = normalize_whitespace(&raw);

    assert_words_in_order(&text, &["ALPHA", "BETA", "GAMMA"], "upright run on a mixed page");
    assert_words_in_order(&text, &READING_ORDER_WORDS, "rotated run on a mixed page");
    assert!(
        text.contains(REPAIRED_PHRASE),
        "the rotated run on a mixed page must still read {REPAIRED_PHRASE:?}; text was {text:?}"
    );
    // Contiguous, in order, and untouched by the rotated frame beside it. Glued rather
    // than space-separated because that is what the upright path emits (see
    // `UPRIGHT_PHRASE_AS_PDF_OXIDE_EMITS_IT`); what this pins is that the repair neither
    // reordered it nor dragged any of its words into the rotated run.
    assert!(
        text.contains("ALPHABETAGAMMA"),
        "the upright run on a mixed page must stay contiguous and in its own order; text was {text:?}"
    );
}

/// Fragments must be joined with a separator, never concatenated.
///
/// The other half of GH#1358 is gluing: rebuilding a run by bare concatenation loses the
/// space the original adjacency supplied (`Engineoil`). Checked separately from ordering
/// because a fix for one has historically shipped without the other.
///
/// 180 is excluded: `pdf_oxide` hands that run over as one already-concatenated span, so
/// there are no fragments to join and nothing this crate can separate. See
/// `rotated_180_run_is_assembled_in_reading_order_on_the_default_config` for the upstream
/// root cause; when that test comes off `#[ignore]`, add 180 back to this list.
#[tokio::test]
async fn rotated_run_fragments_are_not_glued_together() {
    for rotation in [90_u32, 270] {
        let text = normalize_whitespace(&extract_pdf_text(rotated_run_pdf(rotation), "rotated").await);
        for glued in ["ENGINEOIL", "OILLEVEL", "LEVELOIL", "OILENGINE", "ENGINEOILLEVEL"] {
            assert!(
                !text.contains(glued),
                "the {rotation}-degree rotated run glued two fragments into {glued:?}; text was {text:?}"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Safety: unrotated text must be untouched
// ═══════════════════════════════════════════════════════════════════

/// The tightest safety pin available without a corpus: an upright page's exact words.
///
/// Full-text equality is not asserted because the markdown writer owns line breaking and
/// that is not what this work may change. The whitespace-normalised word sequence *is*
/// pinned exactly, which is what a leaked rotated-frame reorder or a lost separator would
/// break.
#[tokio::test]
async fn unrotated_page_text_is_exactly_the_expected_words() {
    let text = normalize_whitespace(&extract_pdf_text(upright_run_pdf(), "upright control").await);

    assert_eq!(
        text, UPRIGHT_PHRASE_AS_PDF_OXIDE_EMITS_IT,
        "an upright page whose words are already in reading order must extract exactly as \
         it did before the rotation repair — {UPRIGHT_PHRASE_AS_PDF_OXIDE_EMITS_IT:?}, glued \
         by upstream #847 — and nothing else. Any drift here invalidates the ~173 calibrated \
         thresholds in pdf_markdown_regression.rs. Reading {REPAIRED_PHRASE:?} instead is not \
         a failure of this crate: it means upstream fixed the inter-word spacing and this \
         baseline needs re-pinning."
    );
}

/// The same property against a real, wholly unrotated 9-page corpus document.
///
/// `pdfa_018.pdf` was chosen because it is already a gated document in
/// `pdf_markdown_regression.rs` (threshold 0.92, measured 0.993) and because every one of
/// its 224 text lines reports `dir="1 0"` — it holds zero rotated spans, so the repair
/// must be a no-op on it by construction.
///
/// The tokens below were read out of the fixture's own extracted text (each is unique in
/// it, none straddles a hyphenation break) and are listed in document order. Asserting
/// their relative order rather than a byte-for-byte snapshot keeps the test from pinning
/// markdown line breaking, while still failing loudly on any reordering, duplication or
/// dropping of unrotated text.
#[tokio::test]
async fn unrotated_corpus_document_keeps_its_tokens_in_document_order() {
    let Some(path) = corpus_fixture("test_documents/pdf/pdfa_018.pdf") else {
        return;
    };
    let text = normalize_whitespace(&extract_corpus_text(&path).await);

    /// Unique tokens of `pdfa_018.pdf`, in the order the document presents them.
    const ORDERED_TOKENS: [&str; 14] = [
        "looking",
        "NURSING",
        "Recommend",
        "Assessment",
        "Specifications",
        "Visceral",
        "Mefenamic",
        "Structured",
        "Publication",
        "Decision",
        "programs",
        "Sighted",
        "Efforts",
        "Identified",
    ];

    assert_words_in_order(&text, &ORDERED_TOKENS, "unrotated corpus document pdfa_018.pdf");
}

// ═══════════════════════════════════════════════════════════════════
// Real-world rotated runs (non-regression)
// ═══════════════════════════════════════════════════════════════════

/// A real vertical arXiv stamp must survive the repair intact.
///
/// Honest limitation: this stamp is a single `Tj`, so it cannot distinguish a fixed build
/// from a broken one — no reordering is possible within one fragment. It is here as a
/// damage detector: the repair must not split, reverse or glue a real rotated run it has
/// nothing to fix.
#[tokio::test]
async fn real_rotated_arxiv_stamp_survives_the_repair_unchanged() {
    let Some(path) = corpus_fixture("test_documents/pdf/tatr.pdf") else {
        return;
    };
    let text = normalize_whitespace(&extract_corpus_text(&path).await);

    assert_words_in_order(
        &text,
        &["arXiv:2110.00061v3", "cs.LG", "18 Nov 2021"],
        "rotated arXiv stamp on page 1 of tatr.pdf",
    );
}

/// The Federal Register side stamp appears once per page; every copy must be intact.
///
/// The expected count is not a hardcoded number: it is derived from the document itself
/// by counting the stamp's unique token. If the repair mangles even one of the seven
/// occurrences, the two counts diverge.
#[tokio::test]
async fn every_rotated_side_stamp_in_a_real_document_stays_intact() {
    let Some(path) = corpus_fixture("test_documents/pdf/2026-13845.pdf") else {
        return;
    };
    let text = normalize_whitespace(&extract_corpus_text(&path).await);

    const STAMP_PHRASE: &str = "lotter on DSK8BHNXB4PROD with PROPOSALS1";
    const STAMP_UNIQUE_TOKEN: &str = "DSK8BHNXB4PROD";

    let token_occurrences = text.matches(STAMP_UNIQUE_TOKEN).count();
    assert!(
        token_occurrences > 0,
        "the rotated side stamp is missing entirely from 2026-13845.pdf; \
         extraction produced {} characters",
        text.len()
    );
    assert_eq!(
        text.matches(STAMP_PHRASE).count(),
        token_occurrences,
        "every occurrence of {STAMP_UNIQUE_TOKEN:?} must sit inside an intact \
         {STAMP_PHRASE:?}; {token_occurrences} token(s) but only {} intact stamp(s)",
        text.matches(STAMP_PHRASE).count()
    );
}

// ═══════════════════════════════════════════════════════════════════
// Corpus presence
// ═══════════════════════════════════════════════════════════════════

/// A corpus-less run must be visibly incomplete, not silently green.
///
/// Every corpus-backed test above skips with a note when its fixture is missing, which is
/// how a fixture-guarded suite has previously stayed green for months without ever
/// running an assertion. This test is the tripwire: if not one of the fixtures is on
/// disk, the run fails and names the fetch command.
#[test]
fn corpus_fixtures_for_rotation_repair_must_be_present() {
    let missing: Vec<&str> = CORPUS_FIXTURES
        .iter()
        .copied()
        .filter(|relative| !repo_root().join(relative).is_file())
        .collect();

    assert!(
        missing.len() < CORPUS_FIXTURES.len(),
        "none of the {} corpus fixtures this suite needs are present, so every \
         corpus-backed rotation test above skipped and proved nothing. Missing: {missing:?}. \
         Run `python3 test_documents/scripts/fetch_corpus.py`.",
        CORPUS_FIXTURES.len()
    );

    if !missing.is_empty() {
        eprintln!(
            "WARNING: {} of {} corpus fixtures missing: {missing:?}",
            missing.len(),
            CORPUS_FIXTURES.len()
        );
    }
}
