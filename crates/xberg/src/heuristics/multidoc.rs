//! Multi-document PDF boundary detection.
//!
//! Provides heuristics to detect where one document ends and another begins
//! within a single PDF file.  Used for fan-out orchestration of N-document
//! PDFs into N per-document jobs.
//!
//! # Detection rules
//!
//! 1. **Page-one marker** — page N+1's text contains "page 1" or "1 of N" pattern
//!    (case-insensitive) → strong boundary (confidence 0.9).
//! 2. **Letterhead reset** — page N has a signature block AND page N+1 starts with
//!    letterhead-like content → strong boundary (0.85).
//! 3. **Density shift** — adjacent pages differ by `> density_shift_threshold` AND
//!    text excerpts share < 10 % common bigrams → weak boundary (0.5).
//! 4. **No signal** → no boundary.
//!
//! # Entry points
//!
//! | Function | When to use |
//! |----------|-------------|
//! | [`detect_boundaries`] | Low-level: caller already has [`MultidocInput`] + [`PageSignals`] |
//! | [`boundaries_from_extraction_result`] | High-level: derive signals from an [`ExtractedDocument`](crate::types::ExtractedDocument) |
//! | [`PageSignals::from_page_text`] | Helper: build signals from plain page text |

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Characters taken from the start of each page for `text_excerpt`.
const TEXT_EXCERPT_LEN: usize = 500;

/// Number of leading lines inspected for letterhead-like detection.
const LETTERHEAD_LINES_CHECKED: usize = 5;

/// Minimum fraction of UPPERCASE characters in a line for it to count as
/// "all-caps" (letterhead style).  0.7 means ≥ 70 % of ASCII letters are upper.
const LETTERHEAD_CAPS_RATIO: f32 = 0.7;

/// Number of leading non-empty lines that must satisfy the caps/short criterion
/// for `starts_with_letterhead_like` to be `true`.
const LETTERHEAD_QUALIFYING_LINES: usize = 2;

/// Maximum character length of a "short" line for letterhead detection.
/// Lines longer than this are not considered title-like regardless of
/// capitalisation.
const LETTERHEAD_SHORT_LINE_MAX: usize = 60;

/// Maximum character offset (in the text) within which a "page 1" marker
/// must appear to qualify as `has_page_number_one_marker`.  Prevents
/// mid-document occurrences from triggering a false positive.
///
/// This is intentionally generous (2 000 chars) to handle documents with
/// lengthy preambles before their page numbering.  Tighten as needed.
const PAGE_ONE_MARKER_WINDOW: usize = 2_000;

/// Signature-related keywords searched in the full page text (lowercased).
const SIGNATURE_KEYWORDS: &[&str] = &[
    "sincerely",
    "regards",
    "yours truly",
    "yours faithfully",
    "signed",
    "signature",
    "/s/",
];

/// Minimum character length of a line that may look like "Name  Date" at the
/// end of a signature block (short lines near signature keywords).
const SIGNATURE_SHORT_LINE_MIN: usize = 3;

/// Maximum character length of a short line following a signature keyword,
/// used to detect "Name / Date" pattern.
const SIGNATURE_SHORT_LINE_MAX: usize = 80;

/// Input signals for multi-document boundary detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultidocInput {
    /// Total number of pages in the PDF.
    pub page_count: u32,
    /// Per-page signals extracted from the PDF.
    pub pages: Vec<PageSignals>,
}

/// Per-page signals extracted from PDF content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSignals {
    /// 1-indexed page number.
    pub page_number: u32,
    /// First ~500 characters of extracted text.
    pub text_excerpt: String,
    /// `true` if page starts with letterhead-like content (ALL CAPS line in first 5 lines
    /// or a logo-image bbox at top).
    pub starts_with_letterhead_like: bool,
    /// `true` if text contains "Page 1" or "1 of N" pattern.
    pub has_page_number_one_marker: bool,
    /// `true` if text contains signature indicators ("Sincerely", "Signed") or
    /// a signature image bbox.
    pub has_signature_block: bool,
    /// Text density: characters per page area, normalised to `[0.0, 1.0]`.
    pub layout_text_density: f32,
}

/// Detected document boundary within a PDF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentBoundary {
    /// 1-indexed start page (inclusive).
    pub start_page: u32,
    /// 1-indexed end page (inclusive).
    pub end_page: u32,
    /// Confidence in this boundary, `[0.0, 1.0]`.
    pub confidence: f32,
    /// Reason for the boundary detection.
    pub reason: BoundaryReason,
}

/// Reason for boundary detection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryReason {
    /// Start of PDF.
    Start,
    /// Page-one marker ("Page 1", "1 of N") detected.
    PageOneMarker,
    /// Letterhead reset after signature block.
    LetterheadReset,
    /// Text density shift with low bigram overlap.
    DensityShift,
    /// End of PDF.
    End,
}

/// Thresholds for multi-document boundary detection.
///
/// All fields are public; callers override any subset via struct-update syntax.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultidocThresholds {
    /// Text density difference threshold for `DensityShift` detection.
    /// Default: 0.3.
    pub density_shift_threshold: f32,
    /// Minimum bigram-overlap ratio below which a density shift is promoted to
    /// a `DensityShift` boundary.  Default: 0.1 (10 % overlap).
    pub bigram_overlap_min: f32,
}

impl Default for MultidocThresholds {
    fn default() -> Self {
        Self {
            density_shift_threshold: 0.3,
            bigram_overlap_min: 0.1,
        }
    }
}

impl PageSignals {
    /// Derive signals from raw page text.
    ///
    /// Callers that already have structured per-page data (e.g. from a PDF extractor)
    /// can set individual fields directly.  This constructor is for callers that only
    /// have the plain-text content of a page (e.g. from [`crate::types::PageContent`]).
    ///
    /// # Arguments
    ///
    /// * `page_number` — 1-indexed page number.
    /// * `text` — Full extracted text for the page.
    /// * `layout_text_density` — Pre-computed text density in `[0.0, 1.0]`.  Pass `0.0`
    ///   when unknown (disables density-shift detection for this page).
    ///
    /// # Heuristics
    ///
    /// All signal derivations are *conservative starting points*.  Each is documented
    /// inline.  They err on the side of fewer false positives; tune thresholds via
    /// [`MultidocThresholds`] rather than by changing these heuristics.
    pub fn from_page_text(page_number: u32, text: &str, layout_text_density: f32) -> Self {
        let text_excerpt = text.chars().take(TEXT_EXCERPT_LEN).collect::<String>();

        Self {
            page_number,
            text_excerpt,
            starts_with_letterhead_like: detect_letterhead(text),
            has_page_number_one_marker: detect_page_one_marker(text),
            has_signature_block: detect_signature_block(text),
            layout_text_density,
        }
    }
}

/// Detect letterhead-like content at the top of a page.
///
/// A page "starts with letterhead" when at least [`LETTERHEAD_QUALIFYING_LINES`] of
/// the first [`LETTERHEAD_LINES_CHECKED`] non-empty lines are *both* short (≤
/// [`LETTERHEAD_SHORT_LINE_MAX`] characters) *and* predominantly uppercase
/// (≥ [`LETTERHEAD_CAPS_RATIO`] of their ASCII letters).
///
/// This matches patterns like:
/// ```text
/// ACME CORPORATION
/// Legal Department
/// ```
/// while rejecting long body paragraphs that happen to be ALL-CAPS.
fn detect_letterhead(text: &str) -> bool {
    let qualifying = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(LETTERHEAD_LINES_CHECKED)
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.len() > LETTERHEAD_SHORT_LINE_MAX {
                return false;
            }
            let ascii_letters = trimmed.chars().filter(|c| c.is_ascii_alphabetic());
            let total: usize = ascii_letters.clone().count();
            if total == 0 {
                return false;
            }
            let upper: usize = ascii_letters.filter(|c| c.is_ascii_uppercase()).count();
            (upper as f32 / total as f32) >= LETTERHEAD_CAPS_RATIO
        })
        .count();

    qualifying >= LETTERHEAD_QUALIFYING_LINES
}

/// Detect a "page 1" or "1 of N" marker within the first [`PAGE_ONE_MARKER_WINDOW`]
/// characters of the page text.
///
/// Matching is case-insensitive.  Checks:
/// - `"page 1"` (standalone via word-boundary-like suffix check: followed by
///   non-digit or end-of-window)
/// - `"1 of "` (common "1 of N" pattern, case-insensitive)
fn detect_page_one_marker(text: &str) -> bool {
    let window: String = text.chars().take(PAGE_ONE_MARKER_WINDOW).collect();
    let lower = window.to_ascii_lowercase();

    if lower.contains("1 of ") {
        return true;
    }

    let mut search = lower.as_str();
    while let Some(pos) = search.find("page 1") {
        let after = &search[pos + "page 1".len()..];
        let next_char = after.chars().next();
        match next_char {
            None => return true,
            Some(c) if !c.is_ascii_digit() => return true,
            _ => {}
        }
        search = &search[pos + 1..];
    }

    false
}

/// Detect a signature block anywhere on the page.
///
/// Returns `true` when the lowercase text contains any of [`SIGNATURE_KEYWORDS`]
/// **and** there is at least one short line (between [`SIGNATURE_SHORT_LINE_MIN`]
/// and [`SIGNATURE_SHORT_LINE_MAX`] characters) near the end of the page, which
/// is typical of "Signed by: Alice Smith  2024-01-15" closings.
///
/// The two-condition check reduces false positives on documents that merely
/// mention "signature" in body prose without being a real closing block.
fn detect_signature_block(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();

    let has_keyword = SIGNATURE_KEYWORDS.iter().any(|kw| lower.contains(kw));
    if !has_keyword {
        return false;
    }

    text.lines().rev().take(10).any(|line| {
        let len = line.trim().len();
        (SIGNATURE_SHORT_LINE_MIN..=SIGNATURE_SHORT_LINE_MAX).contains(&len)
    })
}

/// Derive document boundaries from an already-produced [`ExtractedDocument`](crate::types::ExtractedDocument).
///
/// Builds a [`MultidocInput`] from `result.pages` (one [`PageSignals`] per
/// [`crate::types::PageContent`] entry), then delegates to [`detect_boundaries`].
///
/// # Fallback behaviour
///
/// - If `result.pages` is `None` or empty the whole document is treated as a
///   single document: returns `[Start(1), End(1)]`, matching the contract of
///   [`detect_boundaries`] for a one-page input.
///
/// # Text density
///
/// [`crate::types::PageContent`] does not carry a pre-computed density score.
/// This function approximates density from the *amount* of visible text on the
/// page (see [`approximate_text_density`]): a saturating curve over the
/// non-whitespace character count, so a sparse page (a one-page memo or form)
/// scores low and a dense page (paper body text) scores high.  A raw
/// non-whitespace *ratio* was near-constant across text pages and left the
/// `DensityShift` rule inert.  Pass a custom [`MultidocInput`] to
/// [`detect_boundaries`] directly when you need a higher-fidelity measurement
/// (e.g. chars-per-pt² from a PDF extractor that also knows page geometry).
///
/// # Arguments
///
/// * `result` — Extraction result whose `pages` field will be used.
/// * `thresholds` — Detection thresholds forwarded to [`detect_boundaries`].
pub fn boundaries_from_extraction_result(
    result: &crate::types::ExtractedDocument,
    thresholds: &MultidocThresholds,
) -> Vec<DocumentBoundary> {
    let pages = match result.pages.as_deref() {
        None | Some([]) => {
            return detect_boundaries(
                &MultidocInput {
                    page_count: 1,
                    pages: vec![PageSignals {
                        page_number: 1,
                        text_excerpt: result.content.chars().take(TEXT_EXCERPT_LEN).collect(),
                        starts_with_letterhead_like: false,
                        has_page_number_one_marker: false,
                        has_signature_block: false,
                        layout_text_density: 0.0,
                    }],
                },
                thresholds,
            );
        }
        Some(pages) => pages,
    };

    let page_count = pages.len() as u32;
    let signals: Vec<PageSignals> = pages
        .iter()
        .map(|page| {
            let density = approximate_text_density(&page.content);
            PageSignals::from_page_text(page.page_number, &page.content, density)
        })
        .collect();

    detect_boundaries(
        &MultidocInput {
            page_count,
            pages: signals,
        },
        thresholds,
    )
}

/// Density scale (visible chars): a page reaches ~0.63 density at this many
/// non-whitespace characters and saturates toward 1.0 beyond it. Chosen so a
/// short memo/form page (a few hundred visible chars) scores well below a dense
/// paper body page (thousands), producing a large delta at a document seam.
const DENSITY_CHAR_SCALE: f32 = 800.0;

/// Approximate per-page text density on a saturating `[0.0, 1.0]` scale from the
/// count of visible (non-whitespace) characters.
///
/// `density = 1 - exp(-visible_chars / DENSITY_CHAR_SCALE)`. This measures *how
/// much* text a page carries, which discriminates sparse pages (memos, forms,
/// cover pages) from dense body pages — unlike the non-whitespace *ratio*, which
/// is ~0.8 on virtually every text page and never triggered the `DensityShift`
/// rule. Consecutive pages within one document have similar amounts of text, so
/// their delta stays small; the strict bigram-overlap gate in
/// [`detect_page_transition`] further suppresses intra-document false positives.
///
/// Returns `0.0` for empty strings.
fn approximate_text_density(text: &str) -> f32 {
    let visible = text.chars().filter(|c| !c.is_whitespace()).count() as f32;
    (1.0 - (-visible / DENSITY_CHAR_SCALE).exp()).clamp(0.0, 1.0)
}

/// Detect document boundaries in a multi-document PDF.
///
/// Returns a list of detected boundaries, always including implicit boundaries
/// at start (page 1) and end (page_count).  Boundaries are returned in ascending
/// order of `start_page`.
///
/// # Arguments
///
/// * `input` - Page signals for the PDF
/// * `thresholds` - Detection thresholds
///
/// # Returns
///
/// Ordered list of document boundaries.
pub fn detect_boundaries(input: &MultidocInput, thresholds: &MultidocThresholds) -> Vec<DocumentBoundary> {
    if input.page_count == 0 || input.pages.is_empty() {
        return vec![];
    }

    let mut boundaries = vec![DocumentBoundary {
        start_page: 1,
        end_page: 1,
        confidence: 1.0,
        reason: BoundaryReason::Start,
    }];

    for i in 0..input.pages.len().saturating_sub(1) {
        let current = &input.pages[i];
        let next = &input.pages[i + 1];

        if let Some(boundary) = detect_page_transition(current, next, thresholds) {
            boundaries.push(boundary);
        }
    }

    if input.page_count > 0 {
        boundaries.push(DocumentBoundary {
            start_page: input.page_count,
            end_page: input.page_count,
            confidence: 1.0,
            reason: BoundaryReason::End,
        });
    }

    boundaries
}

/// Detect a boundary between two consecutive pages.
fn detect_page_transition(
    current: &PageSignals,
    next: &PageSignals,
    thresholds: &MultidocThresholds,
) -> Option<DocumentBoundary> {
    if next.has_page_number_one_marker || has_page_one_pattern(&next.text_excerpt) {
        return Some(DocumentBoundary {
            start_page: next.page_number,
            end_page: next.page_number,
            confidence: 0.9,
            reason: BoundaryReason::PageOneMarker,
        });
    }

    if current.has_signature_block && next.starts_with_letterhead_like {
        return Some(DocumentBoundary {
            start_page: next.page_number,
            end_page: next.page_number,
            confidence: 0.85,
            reason: BoundaryReason::LetterheadReset,
        });
    }

    let density_delta = (current.layout_text_density - next.layout_text_density).abs();
    if density_delta > thresholds.density_shift_threshold {
        let overlap_ratio = compute_bigram_overlap(&current.text_excerpt, &next.text_excerpt);
        if overlap_ratio < thresholds.bigram_overlap_min {
            return Some(DocumentBoundary {
                start_page: next.page_number,
                end_page: next.page_number,
                confidence: 0.5,
                reason: BoundaryReason::DensityShift,
            });
        }
    }

    None
}

/// Compute bigram overlap ratio between two text excerpts.
///
/// Returns a value in `[0.0, 1.0]`; 0.0 = no overlap, 1.0 = identical.
fn compute_bigram_overlap(text_a: &str, text_b: &str) -> f32 {
    let bigrams_a = extract_bigrams(text_a);
    let bigrams_b = extract_bigrams(text_b);

    if bigrams_a.is_empty() || bigrams_b.is_empty() {
        return 0.0;
    }

    let intersection = bigrams_a.intersection(&bigrams_b).count();
    let union_size = bigrams_a.len() + bigrams_b.len() - intersection;

    if union_size == 0 {
        0.0
    } else {
        intersection as f32 / union_size as f32
    }
}

/// Extract bigrams (2-character sequences) from text, lowercased and trimmed.
fn extract_bigrams(text: &str) -> HashSet<String> {
    let normalized = text.to_ascii_lowercase();
    let chars: Vec<char> = normalized.chars().collect();

    (0..chars.len().saturating_sub(1))
        .map(|i| format!("{}{}", chars[i], chars[i + 1]))
        .collect()
}

/// Check if text contains "page 1" or "1 of N" pattern (case-insensitive).
fn has_page_one_pattern(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("page 1") || lower.contains("1 of ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page(
        page_number: u32,
        text_excerpt: &str,
        starts_with_letterhead_like: bool,
        has_page_number_one_marker: bool,
        has_signature_block: bool,
        layout_text_density: f32,
    ) -> PageSignals {
        PageSignals {
            page_number,
            text_excerpt: text_excerpt.to_string(),
            starts_with_letterhead_like,
            has_page_number_one_marker,
            has_signature_block,
            layout_text_density,
        }
    }

    #[test]
    fn density_is_sparse_for_short_pages_and_dense_for_long_pages() {
        let empty = approximate_text_density("");
        let sparse = approximate_text_density(&"word ".repeat(40));
        let dense = approximate_text_density(&"word ".repeat(1000));
        assert_eq!(empty, 0.0, "empty page has zero density");
        assert!(sparse < 0.3, "a short memo page should score low, got {sparse}");
        assert!(dense > 0.9, "a dense paper page should saturate high, got {dense}");
        assert!(
            dense - sparse > MultidocThresholds::default().density_shift_threshold,
            "sparse→dense delta must exceed the density-shift threshold"
        );
    }

    #[test]
    fn density_delta_between_two_dense_pages_stays_small() {
        let page_a = approximate_text_density(&"lorem ipsum ".repeat(300));
        let page_b = approximate_text_density(&"dolor sit amet ".repeat(300));
        assert!(
            (page_a - page_b).abs() < MultidocThresholds::default().density_shift_threshold,
            "dense→dense delta {} must stay below the threshold",
            (page_a - page_b).abs()
        );
    }

    #[test]
    fn from_page_text_detects_letterhead() {
        let text = "ACME CORPORATION\nLEGAL DEPT\nThis is body text about something.\n";
        let signals = PageSignals::from_page_text(1, text, 0.5);
        assert!(
            signals.starts_with_letterhead_like,
            "Expected letterhead detection for all-caps short lines"
        );
    }

    #[test]
    fn from_page_text_no_letterhead_for_long_lines() {
        let long_line = "THIS IS A VERY LONG LINE THAT EXCEEDS THE MAXIMUM LETTERHEAD LENGTH THRESHOLD BY FAR";
        let text = format!("{long_line}\n{long_line}\nBody text follows.");
        let signals = PageSignals::from_page_text(1, &text, 0.5);
        assert!(
            !signals.starts_with_letterhead_like,
            "Long ALL-CAPS lines should not trigger letterhead detection"
        );
    }

    #[test]
    fn from_page_text_detects_page_one_marker() {
        let text = "Page 1 of 5\nThis is a document.";
        let signals = PageSignals::from_page_text(2, text, 0.5);
        assert!(signals.has_page_number_one_marker, "Expected page-one marker detection");
    }

    #[test]
    fn from_page_text_page_one_marker_one_of_n() {
        let text = "1 of 10\nDocument content here.";
        let signals = PageSignals::from_page_text(3, text, 0.5);
        assert!(signals.has_page_number_one_marker, "Expected '1 of N' marker detection");
    }

    #[test]
    fn from_page_text_no_page_one_marker_for_page_10() {
        let text = "Page 10 of 20\nDocument body text.";
        let signals = PageSignals::from_page_text(10, text, 0.5);
        assert!(
            !signals.has_page_number_one_marker,
            "page 10 must not trigger page-one marker"
        );
    }

    #[test]
    fn from_page_text_detects_signature_block() {
        let text = "Thank you for your business.\n\nSincerely,\nJohn Smith\n2024-01-15";
        let signals = PageSignals::from_page_text(1, text, 0.5);
        assert!(signals.has_signature_block, "Expected signature block detection");
    }

    #[test]
    fn from_page_text_detects_signature_slash_s() {
        let text = "Agreement is hereby acknowledged.\n\n/s/ Jane Doe\nChief Executive Officer";
        let signals = PageSignals::from_page_text(1, text, 0.5);
        assert!(signals.has_signature_block, "Expected /s/ signature detection");
    }

    #[test]
    fn from_page_text_no_signature_for_body_prose() {
        let text = "Please provide your signature on the form attached. This is a long line that \
                    exceeds the short-line threshold so it should not be treated as a closing block \
                    in the signature detection heuristic.";
        let signals = PageSignals::from_page_text(1, text, 0.5);
        assert!(
            !signals.has_signature_block,
            "Body prose mentioning 'signature' without short closing lines should not trigger"
        );
    }

    #[test]
    fn from_page_text_text_excerpt_truncated() {
        let long_text: String = "x".repeat(1000);
        let signals = PageSignals::from_page_text(1, &long_text, 0.5);
        assert_eq!(
            signals.text_excerpt.len(),
            TEXT_EXCERPT_LEN,
            "text_excerpt should be truncated to TEXT_EXCERPT_LEN"
        );
    }

    fn make_extraction_result(pages: Vec<(&str, u32)>) -> crate::types::ExtractedDocument {
        use crate::types::PageContent;

        crate::types::ExtractedDocument {
            content: pages.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("\n"),
            pages: Some(
                pages
                    .into_iter()
                    .map(|(text, page_number)| PageContent {
                        page_number,
                        content: text.to_string(),
                        tables: vec![],
                        image_indices: vec![],
                        image_preprocessing: None,
                        hierarchy: None,
                        is_blank: None,
                        layout_regions: None,
                        speaker_notes: None,
                        section_name: None,
                        sheet_name: None,
                        ocr_confidence: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn boundaries_from_result_three_pages_detects_second_doc() {
        let result = make_extraction_result(vec![
            ("First document body text for page one.", 1),
            ("Page 1 of 3\nACME CORP\nSecond document header content.", 2),
            ("Continuation of second document.", 3),
        ]);

        let thresholds = MultidocThresholds::default();
        let boundaries = boundaries_from_extraction_result(&result, &thresholds);

        let page_2_boundary = boundaries.iter().find(|b| b.start_page == 2);
        assert!(
            page_2_boundary.is_some(),
            "Expected a boundary at page 2 (new document starts)"
        );
        assert_eq!(page_2_boundary.unwrap().reason, BoundaryReason::PageOneMarker);
        assert!(
            (page_2_boundary.unwrap().confidence - 0.9).abs() < f32::EPSILON,
            "Page-one marker should have confidence 0.9"
        );
    }

    #[test]
    fn boundaries_from_result_single_page_returns_start_end() {
        let result = make_extraction_result(vec![("Single page document.", 1)]);

        let thresholds = MultidocThresholds::default();
        let boundaries = boundaries_from_extraction_result(&result, &thresholds);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].reason, BoundaryReason::Start);
        assert_eq!(boundaries[1].reason, BoundaryReason::End);
    }

    #[test]
    fn boundaries_from_result_no_pages_returns_start_end() {
        let result = crate::types::ExtractedDocument {
            content: "Whole document as one blob.".to_string(),
            pages: None,
            ..Default::default()
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = boundaries_from_extraction_result(&result, &thresholds);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].reason, BoundaryReason::Start);
        assert_eq!(boundaries[1].reason, BoundaryReason::End);
    }

    #[test]
    fn boundaries_from_result_empty_pages_returns_start_end() {
        let result = crate::types::ExtractedDocument {
            content: "Document content.".to_string(),
            pages: Some(vec![]),
            ..Default::default()
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = boundaries_from_extraction_result(&result, &thresholds);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].reason, BoundaryReason::Start);
        assert_eq!(boundaries[1].reason, BoundaryReason::End);
    }

    #[test]
    fn boundaries_from_result_letterhead_after_signature_detected() {
        let result = make_extraction_result(vec![
            ("Dear Customer,\n\nPlease review our offer.\n\nSincerely,\nAlice", 1),
            ("ACME CORP\nINVOICE DEPT\nNew document starts here.", 2),
        ]);

        let thresholds = MultidocThresholds::default();
        let boundaries = boundaries_from_extraction_result(&result, &thresholds);

        let page_2_boundary = boundaries.iter().find(|b| b.start_page == 2);
        assert!(
            page_2_boundary.is_some(),
            "Expected boundary at page 2 (letterhead after signature)"
        );
        assert_eq!(page_2_boundary.unwrap().reason, BoundaryReason::LetterheadReset);
    }

    #[test]
    fn test_single_page_input() {
        let input = MultidocInput {
            page_count: 1,
            pages: vec![sample_page(1, "Hello world", false, false, false, 0.5)],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].reason, BoundaryReason::Start);
        assert_eq!(boundaries[1].reason, BoundaryReason::End);
    }

    #[test]
    fn test_invoice_receipt_scenario() {
        let input = MultidocInput {
            page_count: 5,
            pages: vec![
                sample_page(1, "Invoice #12345. Total: $500", false, false, false, 0.6),
                sample_page(2, "Thank you. Sincerely, John Doe", false, false, true, 0.4),
                sample_page(3, "Receipt. Page 1 of 3. ACME Corp header", true, true, false, 0.7),
                sample_page(4, "Item 1: $10\nItem 2: $20", false, false, false, 0.65),
                sample_page(5, "Total: $30. Thank you", false, false, false, 0.5),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_3_boundaries: Vec<_> = boundaries.iter().filter(|b| b.start_page == 3).collect();
        assert!(!page_3_boundaries.is_empty());

        let strongest = page_3_boundaries.iter().max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        assert_eq!(strongest.unwrap().reason, BoundaryReason::PageOneMarker);
    }

    #[test]
    fn test_page_one_marker_detection() {
        let input = MultidocInput {
            page_count: 2,
            pages: vec![
                sample_page(1, "First document text", false, false, false, 0.5),
                sample_page(2, "Page 1 of 5. Second document here", false, true, false, 0.6),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_2_boundary = boundaries
            .iter()
            .find(|b| b.start_page == 2)
            .expect("Should detect boundary at page 2");

        assert_eq!(page_2_boundary.reason, BoundaryReason::PageOneMarker);
        assert_eq!(page_2_boundary.confidence, 0.9);
    }

    #[test]
    fn test_letterhead_reset_detection() {
        let input = MultidocInput {
            page_count: 2,
            pages: vec![
                sample_page(1, "Letter content. Sincerely, John", false, false, true, 0.5),
                sample_page(2, "NEW CORP LETTERHEAD. Invoice header", true, false, false, 0.6),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_2_boundary = boundaries
            .iter()
            .find(|b| b.start_page == 2)
            .expect("Should detect boundary at page 2");

        assert_eq!(page_2_boundary.reason, BoundaryReason::LetterheadReset);
        assert_eq!(page_2_boundary.confidence, 0.85);
    }

    #[test]
    fn test_density_shift_detection() {
        let input = MultidocInput {
            page_count: 2,
            pages: vec![
                sample_page(1, "sparse page text", false, false, false, 0.2),
                sample_page(
                    2,
                    "completely different document content that has nothing in common",
                    false,
                    false,
                    false,
                    0.8,
                ),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_2_boundary = boundaries
            .iter()
            .find(|b| b.start_page == 2)
            .expect("Should detect boundary at page 2 due to density shift");

        assert_eq!(page_2_boundary.reason, BoundaryReason::DensityShift);
        assert_eq!(page_2_boundary.confidence, 0.5);
    }

    #[test]
    fn test_no_boundary_with_high_bigram_overlap() {
        let common_text = "The quick brown fox jumps over the lazy dog";
        let input = MultidocInput {
            page_count: 2,
            pages: vec![
                sample_page(1, common_text, false, false, false, 0.5),
                sample_page(2, common_text, false, false, false, 0.8),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_2_density_shift = boundaries
            .iter()
            .find(|b| b.start_page == 2 && b.reason == BoundaryReason::DensityShift);
        assert!(page_2_density_shift.is_none());
    }

    #[test]
    fn test_priority_page_one_over_letterhead() {
        let input = MultidocInput {
            page_count: 2,
            pages: vec![
                sample_page(1, "Letter. Sincerely", false, false, true, 0.5),
                sample_page(2, "Page 1 of 10. CORP HEADER", true, true, false, 0.6),
            ],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        let page_2_boundary = boundaries
            .iter()
            .find(|b| b.start_page == 2)
            .expect("Should detect boundary at page 2");

        assert_eq!(page_2_boundary.reason, BoundaryReason::PageOneMarker);
        assert_eq!(page_2_boundary.confidence, 0.9);
    }

    #[test]
    fn test_empty_input() {
        let input = MultidocInput {
            page_count: 0,
            pages: vec![],
        };

        let thresholds = MultidocThresholds::default();
        let boundaries = detect_boundaries(&input, &thresholds);

        assert_eq!(boundaries.len(), 0);
    }

    #[test]
    fn test_bigram_overlap_identical_text() {
        let text = "hello world";
        let overlap = compute_bigram_overlap(text, text);
        assert_eq!(overlap, 1.0);
    }

    #[test]
    fn test_bigram_overlap_completely_different() {
        let text_a = "aaaa";
        let text_b = "bbbb";
        let overlap = compute_bigram_overlap(text_a, text_b);
        assert_eq!(overlap, 0.0);
    }

    #[test]
    fn test_bigram_overlap_partial() {
        let text_a = "hello";
        let text_b = "hella";
        let overlap = compute_bigram_overlap(text_a, text_b);
        assert!(overlap > 0.5 && overlap < 0.7);
    }

    #[test]
    fn test_extract_bigrams() {
        let bigrams = extract_bigrams("ab");
        assert_eq!(bigrams.len(), 1);
        assert!(bigrams.contains("ab"));

        let bigrams = extract_bigrams("abc");
        assert_eq!(bigrams.len(), 2);
        assert!(bigrams.contains("ab"));
        assert!(bigrams.contains("bc"));
    }

    #[test]
    fn test_default_thresholds() {
        let thresholds = MultidocThresholds::default();
        assert_eq!(thresholds.density_shift_threshold, 0.3);
        assert_eq!(thresholds.bigram_overlap_min, 0.1);
    }
}
