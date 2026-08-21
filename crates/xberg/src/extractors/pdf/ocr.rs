//! OCR functionality for PDF extraction.
//!
//! Handles text quality evaluation, OCR fallback decision logic, and OCR processing.

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use std::borrow::Cow;

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use crate::core::config::ExtractionConfig;
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use crate::core::config::OcrQualityThresholds;

/// Minimum average non-whitespace characters per page for extracted text to be treated as
/// substantive. At or above this, prose-tuned quality checks (fragmentation, avg word length,
/// consecutive-repeat ratio) are skipped so legitimately non-prose content — numeric tables,
/// formula pages, sparse forms — is not misclassified as needing OCR (issue #1176). Corruption
/// checks (empty, no-alphanumerics, garbage chars, critical fragmentation) still always apply.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const MIN_AVG_NON_WHITESPACE_TO_TRUST: f64 = 150.0;

/// Inclusive start of the Unicode Private Use Area (BMP: U+E000-U+F8FF). Codepoints here
/// have no standard meaning; a font's glyph-index-to-character mapping that resolves into
/// this range signals an undecodable text layer rather than real text (issue #1254).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const PUA_RANGE_START: u32 = 0xE000;

/// Inclusive end of the Unicode Private Use Area (BMP).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const PUA_RANGE_END: u32 = 0xF8FF;

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
type EncodedPage = (usize, std::sync::Arc<Vec<u8>>, u32, u32);

/// Returns `true` for characters that indicate a broken glyph-to-Unicode mapping rather
/// than legible text: Unicode Private Use Area codepoints (a common fallback target for
/// undecodable CID/glyph indices), the replacement character (U+FFFD), and non-whitespace
/// control characters. Ordinary symbols, punctuation, and emoji are unaffected.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn is_undecodable_char(ch: char) -> bool {
    let code = ch as u32;
    (PUA_RANGE_START..=PUA_RANGE_END).contains(&code) || ch == '\u{FFFD}' || (ch.is_control() && !ch.is_whitespace())
}

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Default)]
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub struct NativeTextStats {
    pub non_whitespace: usize,
    pub alnum: usize,
    pub meaningful_words: usize,
    pub alnum_ratio: f64,
    /// Count of Unicode replacement characters (U+FFFD) indicating encoding failures.
    pub garbage_char_count: usize,
    /// Fraction of whitespace-delimited words that are 1-2 characters (0.0-1.0).
    /// High values indicate fragmented/garbled text extraction.
    pub fragmented_word_ratio: f64,
    /// Fraction of consecutive word pairs that are identical (0.0-1.0).
    /// High values indicate column scrambling where text is duplicated.
    pub consecutive_repeat_ratio: f64,
    /// Average word length (by chars). Very low values indicate garbled extraction.
    pub avg_word_length: f64,
    /// Total word count (whitespace-delimited).
    pub word_count: usize,
    /// Fraction of non-whitespace characters that are undecodable — Unicode Private Use
    /// Area, replacement characters, or non-whitespace control characters (0.0-1.0). High
    /// values indicate a text layer whose glyph-to-Unicode mapping is broken (issue #1254),
    /// e.g. a subset `Identity-H`/`CIDToGIDMap /Identity` font with no `/ToUnicode` CMap and
    /// no `cmap`/`post` table to fall back to.
    pub undecodable_ratio: f64,
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub struct OcrFallbackDecision {
    pub stats: NativeTextStats,
    pub avg_non_whitespace: f64,
    pub avg_alnum: f64,
    pub fallback: bool,
    pub failing_pages: Vec<u32>,
    /// Set to `true` when the aggregate document quality check triggered `fallback`,
    /// independently of any per-page analysis. When this is true the gate routes to
    /// `RunFallback` (full OCR) regardless of whether `failing_pages` is populated.
    pub whole_doc_failure: bool,
}

/// Which branch the OCR skip gate selects, given pre-rendered doc presence,
/// text statistics, and the per-page fallback decision.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OcrGateOutcome {
    /// Content is non-textual and a pre-rendered doc is available — skip OCR.
    SkipNonText,
    /// Pre-rendered doc is substantive and no per-page fallback is needed — skip OCR.
    SkipSubstantive,
    /// A document-level quality check flagged the entire document — OCR every page.
    RunFallback,
    /// A per-page quality check flagged a scanned page — run OCR fallback.
    RunFallbackOnPages(Vec<u32>),
    /// Insufficient native text or no structured doc available — use native text.
    UseNative,
}

/// Decide whether to skip OCR, run OCR fallback, or use native text.
///
/// Extracted from the async PDF pipeline so the gate logic can be unit-tested
/// independently. Fixes #917: `has_substantive_doc` alone must not suppress
/// OCR when `decision_fallback` is true (a scanned page was detected despite
/// good aggregate text).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn evaluate_ocr_skip_gate(
    pre_rendered_doc_present: bool,
    total_chars: usize,
    alnum_ws_ratio: f64,
    decision: &OcrFallbackDecision,
    thresholds: &crate::core::config::OcrQualityThresholds,
) -> OcrGateOutcome {
    // The non-text skip is for genuinely non-textual *structured* content (a
    // vector diagram or chart the structured extractor rendered faithfully),
    // where OCR would only add noise. A whole-document quality failure is the
    // opposite: a scan or a garbage/undecodable text layer with no trustworthy
    // native text at all, which must reach OCR regardless of how "non-textual"
    // the stray characters look (issue #1338). Guard it exactly as the
    // substantive-doc branch below guards against `decision.fallback`.
    let skip_for_non_text = pre_rendered_doc_present
        && total_chars >= thresholds.non_text_min_chars
        && alnum_ws_ratio < thresholds.alnum_ws_ratio_threshold
        && !decision.whole_doc_failure;

    let has_substantive_doc = pre_rendered_doc_present
        && total_chars >= thresholds.substantive_min_chars
        && alnum_ws_ratio >= thresholds.alnum_ws_ratio_threshold;

    if skip_for_non_text {
        OcrGateOutcome::SkipNonText
    } else if has_substantive_doc && !decision.fallback {
        OcrGateOutcome::SkipSubstantive
    } else if decision.fallback {
        if decision.whole_doc_failure || decision.failing_pages.is_empty() {
            OcrGateOutcome::RunFallback
        } else {
            OcrGateOutcome::RunFallbackOnPages(decision.failing_pages.clone())
        }
    } else {
        OcrGateOutcome::UseNative
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
impl NativeTextStats {
    pub(crate) fn compute(text: &str, thresholds: &OcrQualityThresholds) -> Self {
        let mut non_whitespace = 0usize;
        let mut alnum = 0usize;
        let mut garbage_char_count = 0usize;
        let mut undecodable_count = 0usize;

        for ch in text.chars() {
            if ch == '\u{FFFD}' {
                garbage_char_count += 1;
            }
            if is_undecodable_char(ch) {
                undecodable_count += 1;
            }
            if !ch.is_whitespace() {
                non_whitespace += 1;
                if ch.is_alphanumeric() {
                    alnum += 1;
                }
            }
        }

        let undecodable_ratio = if non_whitespace == 0 {
            0.0
        } else {
            undecodable_count as f64 / non_whitespace as f64
        };

        let meaningful_words = text
            .split_whitespace()
            .filter(|word| {
                word.chars()
                    .filter(|c| c.is_alphanumeric())
                    .take(thresholds.min_meaningful_word_len)
                    .count()
                    >= thresholds.min_meaningful_word_len
            })
            .count();

        let alnum_ratio = if non_whitespace == 0 {
            0.0
        } else {
            alnum as f64 / non_whitespace as f64
        };

        let words: Vec<&str> = text.split_whitespace().collect();
        let fragmented_word_ratio = if words.len() >= 10 {
            let short_count = words.iter().filter(|w| w.len() <= 2).count();
            short_count as f64 / words.len() as f64
        } else {
            0.0
        };

        let consecutive_repeat_ratio = if words.len() >= thresholds.min_words_for_repeat_check {
            let repeat_count = words.windows(2).filter(|pair| pair[0] == pair[1]).count();
            repeat_count as f64 / (words.len() - 1) as f64
        } else {
            0.0
        };

        let avg_word_length = if words.is_empty() {
            0.0
        } else {
            words.iter().map(|w| w.len()).sum::<usize>() as f64 / words.len() as f64
        };

        Self {
            non_whitespace,
            alnum,
            meaningful_words,
            alnum_ratio,
            garbage_char_count,
            fragmented_word_ratio,
            consecutive_repeat_ratio,
            avg_word_length,
            word_count: words.len(),
            undecodable_ratio,
        }
    }

    /// Convenience method using default thresholds.
    // Gated to `ocr` to match its only callers, which live in the
    // `#[cfg(all(test, feature = "ocr"))]` test module below. `ocr-pipeline`
    // alone (pulled in by `liter-llm`) compiles this file but not them. ~keep
    #[cfg(all(test, feature = "ocr"))]
    pub(crate) fn from(text: &str) -> Self {
        Self::compute(text, &OcrQualityThresholds::default())
    }
}

/// Evaluates native PDF text quality to determine if OCR fallback is needed.
///
/// Uses the provided quality thresholds (or defaults) to make the decision.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn evaluate_native_text_for_ocr(
    native_text: &str,
    page_count: Option<u32>,
    thresholds: &OcrQualityThresholds,
) -> OcrFallbackDecision {
    let trimmed = native_text.trim();

    if trimmed.is_empty() {
        let empty_stats = NativeTextStats {
            non_whitespace: 0,
            alnum: 0,
            meaningful_words: 0,
            alnum_ratio: 0.0,
            garbage_char_count: 0,
            fragmented_word_ratio: 0.0,
            consecutive_repeat_ratio: 0.0,
            avg_word_length: 0.0,
            word_count: 0,
            undecodable_ratio: 0.0,
        };
        return OcrFallbackDecision {
            stats: empty_stats,
            avg_non_whitespace: 0.0,
            avg_alnum: 0.0,
            fallback: true,
            failing_pages: Vec::new(),
            whole_doc_failure: true,
        };
    }

    let stats = NativeTextStats::compute(trimmed, thresholds);
    let pages = page_count.unwrap_or(1).max(1) as f64;
    let avg_non_whitespace = stats.non_whitespace as f64 / pages;
    let avg_alnum = stats.alnum as f64 / pages;

    let has_substantial_text = stats.non_whitespace >= thresholds.min_total_non_whitespace
        && avg_non_whitespace >= thresholds.min_non_whitespace_per_page
        && stats.meaningful_words >= thresholds.min_meaningful_words;

    let has_substantial_content = avg_non_whitespace >= MIN_AVG_NON_WHITESPACE_TO_TRUST;

    // A page with a high fraction of undecodable characters (PUA / replacement / control
    // garbage) has a broken glyph-to-Unicode mapping regardless of how "substantial" the
    // page otherwise looks — it is gated only by a minimum character count so a stray
    // symbol or two on an otherwise short page can't trip it (issue #1254). ~keep
    let has_undecodable_text_layer = stats.non_whitespace >= thresholds.min_total_non_whitespace
        && stats.undecodable_ratio >= thresholds.min_undecodable_ratio;

    let definitive_failure = stats.non_whitespace == 0
        || stats.alnum == 0
        || stats.garbage_char_count >= thresholds.min_garbage_chars
        || stats.fragmented_word_ratio >= thresholds.critical_fragmented_word_ratio
        || has_undecodable_text_layer
        || (!has_substantial_content
            && (stats.fragmented_word_ratio >= thresholds.max_fragmented_word_ratio
                && stats.meaningful_words < thresholds.min_meaningful_words))
        || (!has_substantial_content
            && (stats.avg_word_length < thresholds.min_avg_word_length
                && stats.word_count >= thresholds.min_words_for_avg_length_check))
        || (!has_substantial_content && stats.consecutive_repeat_ratio >= thresholds.min_consecutive_repeat_ratio);

    let fallback = if definitive_failure {
        true
    } else if has_substantial_text {
        false
    } else if (stats.alnum_ratio < thresholds.min_alnum_ratio && avg_alnum < thresholds.min_non_whitespace_per_page)
        || (stats.non_whitespace < thresholds.min_total_non_whitespace
            && avg_non_whitespace < thresholds.min_non_whitespace_per_page)
    {
        true
    } else {
        stats.meaningful_words == 0 && avg_non_whitespace < thresholds.min_non_whitespace_per_page
    };

    OcrFallbackDecision {
        stats,
        avg_non_whitespace,
        avg_alnum,
        fallback,
        failing_pages: Vec::new(),
        whole_doc_failure: fallback,
    }
}

/// Normalize structural Markdown markers out of OCR text **for scoring only**.
///
/// The quality heuristics in [`NativeTextStats`] measure surface text shape
/// (alphanumeric ratio, word length, fragmentation). Structural Markdown — table
/// pipes, heading hashes, list bullets, emphasis, code fences — is non-alphanumeric
/// and tokenizes into short fragments, so a richer, *more accurate* VLM result that
/// emits Markdown scores **lower** than plain prose from a classical backend. That
/// systematically disadvantages the VLM in pipeline selection (#1341).
///
/// This replaces structural punctuation with spaces (dropping it from the
/// non-whitespace denominator) and skips code-fence / table-separator lines, so the
/// score reflects the prose content rather than the formatting. The returned string
/// is used only as scoring input; the emitted OCR text is never altered. Inline
/// hyphens and periods are preserved so real word lengths are unaffected.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn normalize_markdown_for_scoring(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Code-fence markers carry no prose.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            continue;
        }
        // Table separator rows (e.g. `|---|:--:|`) are pure structure.
        let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if !compact.is_empty() && compact.chars().all(|c| matches!(c, '|' | '-' | ':' | '+')) {
            continue;
        }
        // Strip a single leading block marker: heading, blockquote, or list bullet.
        let mut content = trimmed.trim_start_matches('#').trim_start();
        content = content.trim_start_matches('>').trim_start();
        for bullet in ["- ", "* ", "+ "] {
            if let Some(rest) = content.strip_prefix(bullet) {
                content = rest;
                break;
            }
        }
        // Strip an ordered-list marker: a run of digits followed by `.` or `)` and a
        // space (e.g. "1. ", "12) "). Without this, ordered-list-heavy Markdown is
        // penalized the same way unstripped unordered bullets would be.
        let digit_prefix_len = content.chars().take_while(char::is_ascii_digit).count();
        if digit_prefix_len > 0
            && let Some(rest) = content[digit_prefix_len..]
                .strip_prefix(". ")
                .or_else(|| content[digit_prefix_len..].strip_prefix(") "))
        {
            content = rest;
        }
        // Inline structural punctuation becomes whitespace so it leaves the
        // non-whitespace denominator; word-internal '-'/'.' are kept.
        for ch in content.chars() {
            if matches!(ch, '|' | '`' | '*' | '_' | '~' | '#') {
                out.push(' ');
            } else {
                out.push(ch);
            }
        }
        out.push('\n');
    }
    out
}

/// Compute a quality score (0.0-1.0) for OCR output text.
///
/// Used by the pipeline to decide whether to accept a result or try the next backend.
/// Higher is better. Combines multiple signal dimensions into a single score.
/// Neutralization switch for the widened list-marker repair added for #733 (letter-for-digit
/// OCR confusions and the doubled `lL.` misread of a single `1.`). Flip to `false` to A/B this
/// behavior in isolation — the original, narrower repairs (`3,` -> `3.` and `l.` -> `1.`) are
/// unconditional and keep working either way.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
const ENABLE_WIDENED_OCR_LIST_MARKER_REPAIR: bool = true;

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
/// Repair ordered-list markers the OCR engine mis-read at the start of a line.
///
/// A list marker is one or two characters carrying all of the structure: `3.` makes a list
/// item, `3,` makes a paragraph that happens to begin with a digit. Tesseract confuses the
/// period with a comma and the digit one with a lowercase L often enough that a document can
/// lose a quarter of its list structure while every word in it is correct — measured on a
/// recorded ordinance, 4 of 19 markers were destroyed this way and the markdown had 18 list
/// items against the source's 31.
///
/// Two repairs are unconditional because the marker they fix is never a legitimate list
/// letter: `<digits>,` (the period read as a comma) and `l.` / `lL.` (the digit one read as a
/// lowercase L, sometimes split into two characters).
///
/// A third, wider class is gated behind [`ENABLE_WIDENED_OCR_LIST_MARKER_REPAIR`]: single
/// uppercase letters Tesseract confuses with a digit (`L`/`G`/`b`/`S`/`O`/`D`/`I`). These
/// *are* ambiguous — `A.`, `B.`, `G.`, `H.` are legitimate lettered markers in this same
/// corpus, so a lone `G.` cannot be repaired on its own shape alone. The discriminator is the
/// run of list markers around it: this function already sees the whole page (`text`, split
/// into every line), so it classifies every marker-shaped line first — unambiguous numeric
/// (`5.`, `12.`) or unambiguous letter (`A.`, `F.`, any letter not in the confusable set) —
/// and then, for each ambiguous line, looks outward past prose to the nearest classified
/// marker on each side. A `G.` between `5.` and `7.` sits in a numeric run and is repaired to
/// `6.`; a `G.` between `F.` and `H.` sits in a lettered run and is left alone. A `G.` with no
/// determinable neighbor on either side is left alone — the same "decline to judge" posture
/// used elsewhere in this file when a signal is not trustworthy.
///
/// Deliberately narrow otherwise, because rewriting text the engine got right is worse than
/// leaving a marker broken. A line is only ever a repair candidate when it looks like nothing
/// but a list item: the marker is at the very start of the line, the number is 1-2 digits (a
/// real list; `2024, the year ...` is not), and it is followed by exactly one space and then
/// an uppercase letter. Sentences beginning "3, and the remainder ..." keep their comma
/// because of the uppercase requirement; prose beginning "l. " does not occur.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn repair_ocr_list_markers(text: &str) -> std::borrow::Cow<'_, str> {
    let lines: Vec<&str> = text.lines().collect();
    let kinds: Vec<LineMarkerKind> = lines.iter().map(|line| classify_marker_line(line)).collect();

    let mut repair_flags = vec![false; lines.len()];
    let mut any_repair = false;
    for (index, kind) in kinds.iter().enumerate() {
        let repair = match kind {
            LineMarkerKind::LegacyRepairableDigit | LineMarkerKind::LegacyRepairableL => true,
            LineMarkerKind::DoubledOneMisread if ENABLE_WIDENED_OCR_LIST_MARKER_REPAIR => true,
            LineMarkerKind::AmbiguousLetter(_) if ENABLE_WIDENED_OCR_LIST_MARKER_REPAIR => {
                ambiguous_marker_is_numeric_context(&kinds, index)
            }
            _ => false,
        };
        if repair {
            repair_flags[index] = true;
            any_repair = true;
        }
    }

    if !any_repair {
        return std::borrow::Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if repair_flags[index] {
            out.push_str(&repaired_marker_line(line, &kinds[index]));
        } else {
            out.push_str(line);
        }
    }
    if text.ends_with('\n') {
        out.push('\n');
    }
    std::borrow::Cow::Owned(out)
}

/// How a line's leading token reads as an ordered-list marker.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineMarkerKind {
    /// Not a list-marker-shaped line at all.
    None,
    /// `<digits>,` — the period was read as a comma. Never ambiguous, always repaired.
    LegacyRepairableDigit,
    /// `l.` — the digit one was read as a lowercase L. Never a valid marker otherwise, always
    /// repaired.
    LegacyRepairableL,
    /// `lL.` — the digit one split into two mis-read characters. Never a valid marker,
    /// repaired when the widened gate is on.
    DoubledOneMisread,
    /// A single letter Tesseract also produces for a digit (see [`confusable_digit_for_letter`]).
    /// Ambiguous with a genuine lettered marker; only repaired when its neighboring markers on
    /// the page indicate a numeric run.
    AmbiguousLetter(char),
    /// An unambiguous numeric marker (`5.`, `12.`) — already correct, used as context.
    Digit,
    /// An unambiguous letter marker (a letter that is not in the confusable set, e.g. `A.`,
    /// `F.`) — already correct, used as context.
    Letter,
}

/// Classify a line's leading token as an ordered-list marker, if it looks like one at all.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn classify_marker_line(line: &str) -> LineMarkerKind {
    let Some((marker, rest)) = line.split_once(' ') else {
        return LineMarkerKind::None;
    };
    // Exactly one space, then an uppercase letter: the shape of a list item, not of prose.
    if !rest.chars().next().is_some_and(char::is_uppercase) {
        return LineMarkerKind::None;
    }
    if let Some(digits) = marker.strip_suffix(',') {
        // `3,` / `12,` -> the period was read as a comma. ~keep
        if !digits.is_empty() && digits.len() <= 2 && digits.bytes().all(|b| b.is_ascii_digit()) {
            return LineMarkerKind::LegacyRepairableDigit;
        }
        return LineMarkerKind::None;
    }
    if marker == "l." {
        return LineMarkerKind::LegacyRepairableL;
    }
    if marker == "lL." {
        return LineMarkerKind::DoubledOneMisread;
    }
    let Some(body) = marker.strip_suffix('.') else {
        return LineMarkerKind::None;
    };
    if !body.is_empty() && body.len() <= 2 && body.bytes().all(|b| b.is_ascii_digit()) {
        return LineMarkerKind::Digit;
    }
    let mut chars = body.chars();
    if let (Some(ch), None) = (chars.next(), chars.next())
        && ch.is_ascii_alphabetic()
    {
        return match confusable_digit_for_letter(ch) {
            Some(_) => LineMarkerKind::AmbiguousLetter(ch),
            None => LineMarkerKind::Letter,
        };
    }
    LineMarkerKind::None
}

/// The digit Tesseract sometimes mis-reads as this uppercase letter, if any.
///
/// Deliberately the exact confusion set observed on the ordinance fixture, not a general
/// OCR confusion table: widening it further would widen the set of legitimate lettered
/// markers (`A.` .. `Z.`) this function has to reason about being corrupted.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn confusable_digit_for_letter(ch: char) -> Option<char> {
    match ch {
        'L' => Some('1'),
        'G' | 'b' => Some('6'),
        'S' => Some('5'),
        'O' | 'D' => Some('0'),
        'I' => Some('1'),
        _ => None,
    }
}

/// This marker line's contribution, if any, to deciding whether a *neighboring* ambiguous
/// line sits in a numeric or a lettered run.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn nearest_marker_is_digit(kind: &LineMarkerKind) -> Option<bool> {
    match kind {
        LineMarkerKind::Digit | LineMarkerKind::LegacyRepairableDigit | LineMarkerKind::LegacyRepairableL => Some(true),
        LineMarkerKind::Letter => Some(false),
        LineMarkerKind::AmbiguousLetter(_) | LineMarkerKind::DoubledOneMisread | LineMarkerKind::None => None,
    }
}

/// Whether the ambiguous letter marker at `kinds[index]` sits in a numeric list, based on the
/// nearest determinable marker on each side.
///
/// Repairs only when every side that found an answer says "numeric" (a `Letter` neighbor on
/// either side vetoes the repair) and at least one side found an answer at all. A page with no
/// determinable neighbor in either direction is left alone.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ambiguous_marker_is_numeric_context(kinds: &[LineMarkerKind], index: usize) -> bool {
    let before = kinds[..index].iter().rev().find_map(nearest_marker_is_digit);
    let after = kinds[index + 1..].iter().find_map(nearest_marker_is_digit);
    matches!(
        (before, after),
        (Some(true), Some(true)) | (Some(true), None) | (None, Some(true))
    )
}

/// Rewrite a line already decided to be repaired, per its classified kind.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn repaired_marker_line(line: &str, kind: &LineMarkerKind) -> String {
    let space_index = line.find(' ').unwrap_or(0);
    let (marker, rest) = line.split_at(space_index);
    let mut out = String::with_capacity(line.len());
    match kind {
        LineMarkerKind::LegacyRepairableDigit => out.push_str(&marker[..marker.len() - 1]),
        LineMarkerKind::LegacyRepairableL | LineMarkerKind::DoubledOneMisread => out.push('1'),
        LineMarkerKind::AmbiguousLetter(ch) => {
            out.push(confusable_digit_for_letter(*ch).expect("classified as ambiguous only when confusable"));
        }
        LineMarkerKind::None | LineMarkerKind::Digit | LineMarkerKind::Letter => out.push_str(marker),
    }
    out.push('.');
    out.push_str(rest);
    out
}

/// Resolve a backend's confidence semantics from the object the registry holds for it,
/// never from its name.
///
/// `min_ocr_mean_confidence` is an ABSOLUTE 0-100 floor, and a floor only means something
/// against a known scale. Tesseract's `mean_text_conf` tracks legibility: on a recorded
/// ordinance its prose pages scored 89-95 and its scanned drawings 36-62 — that backend
/// reports [`ConfidenceSemantics::Legibility`].
///
/// Sceptre's does not. On the same document every page landed between 36 and 74 — the entire
/// document below a floor calibrated for Tesseract — and the ordering is inverted: the real
/// Plant List page scored 39 while a pure drawing scored 74. Applying the floor there
/// discarded all 16 pages and produced an empty document. Sceptre reports
/// [`ConfidenceSemantics::Uncalibrated`], so the gate never applies to it.
///
/// Prefers the backend object the single-backend route above already resolved. The
/// multi-stage pipeline route resolves a backend per stage internally and never surfaces one
/// to this call site, so when `backend` is `None` this falls back to a fresh registry lookup
/// by the top-level configured name — the same lookup that resolved `backend` above — and
/// asks the returned object directly. That fallback is a known approximation: a pipeline
/// page's actual producing backend can differ per page (whichever stage succeeded for it),
/// and nothing here tracks that per page.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn resolve_confidence_semantics(
    backend: Option<&std::sync::Arc<dyn crate::plugins::OcrBackend>>,
    backend_name: &str,
) -> crate::plugins::ConfidenceSemantics {
    if let Some(backend) = backend {
        return backend.confidence_semantics();
    }
    let registry = crate::plugins::registry::get_ocr_backend_registry();
    let registry = registry.read();
    registry
        .get(backend_name)
        .map(|backend| backend.confidence_semantics())
        .unwrap_or(crate::plugins::ConfidenceSemantics::Uncalibrated)
}

/// The backend-native-scale confidence floor a page's confidence must clear, or `false` if
/// this backend's confidence cannot be used as an absolute gate at all.
///
/// Only [`ConfidenceSemantics::Legibility`] is normalized (by its `scale_max`) and compared
/// against the ABSOLUTE 0-100 `min_ocr_mean_confidence` threshold. `Uncalibrated` and `None`
/// never reject here — their number, if any, does not mean legibility — so callers must fall
/// back to [`is_ocr_recognition_noise`] for those.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn confidence_gate_rejects(
    semantics: crate::plugins::ConfidenceSemantics,
    confidence: Option<f64>,
    min_ocr_mean_confidence: f64,
) -> bool {
    let crate::plugins::ConfidenceSemantics::Legibility { scale_max } = semantics else {
        return false;
    };
    if scale_max <= 0.0 || min_ocr_mean_confidence <= 0.0 {
        return false;
    }
    confidence.is_some_and(|c| c / scale_max < min_ocr_mean_confidence / 100.0)
}

/// Whether the confidence gate is authoritative for this page at all — i.e. whether the
/// backend's semantics are `Legibility` and it actually reported a confidence. When this is
/// `false`, the text-shape heuristic decides instead of the (possibly meaningless) number.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn confidence_gate_applies(semantics: crate::plugins::ConfidenceSemantics, confidence: Option<f64>) -> bool {
    matches!(semantics, crate::plugins::ConfidenceSemantics::Legibility { scale_max } if scale_max > 0.0)
        && confidence.is_some()
}

/// Tesseract's mean confidence for a page, 0-100, if the backend reported one.
///
/// Written by `perform_ocr` from `api.mean_text_conf()`. Backends that do not report it
/// (and Tesseract itself, when it read nothing) simply yield `None`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn mean_text_conf_of(metadata: &ahash::AHashMap<std::borrow::Cow<'_, str>, serde_json::Value>) -> Option<f64> {
    let value = metadata.get("mean_text_conf")?;
    let conf = value.as_f64().or_else(|| value.as_i64().map(|v| v as f64))?;
    // -1 is Tesseract's "no confidence available" sentinel.
    (conf >= 0.0).then_some(conf)
}

/// Statistics for judging an OCR result, scored over prose rather than Markdown scaffolding.
///
/// A table's delimiter rows (`| --- | --- |`) are entirely one-character tokens, so scoring the
/// raw Markdown makes a page of perfectly good tabular OCR look exactly like line-art noise.
/// [`compute_quality_score`] already normalizes for this (#1341); the veto has to as well, or a
/// scanned table becomes its most likely false positive.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ocr_output_stats(text: &str, thresholds: &OcrQualityThresholds) -> NativeTextStats {
    let normalized = normalize_markdown_for_scoring(text.trim());
    let scoring_input = if normalized.trim().is_empty() {
        text.trim()
    } else {
        normalized.as_str()
    };
    NativeTextStats::compute(scoring_input, thresholds)
}

/// Is this OCR result recognition noise rather than page content?
///
/// An OCR engine run over line art — a scanned survey plat, an engineering drawing, the
/// flourish of a signature — does not fail. It returns confident-looking strings that are
/// not words (`MAM RAM SAL, Eid wat au TH.8) FLAT`), and nothing downstream distinguishes
/// them from prose. Before the JBIG2 `/ImageMask` fix such pages rendered blank and OCR'd to
/// nothing, so the noise was invisible; once the masks paint, the drawings are legible to the
/// rasterizer and the engine dutifully "reads" them.
///
/// [`compute_quality_score`] cannot make this call. It is a weighted blend, so one
/// catastrophic signal is diluted by five healthy ones: measured across the 16 pages of a
/// recorded ordinance, pure drawing noise scored 0.81-0.85 against 0.94-0.98 for clean prose.
/// A 0.09 separation is not something to threshold on. A *rejection* decision needs a veto on
/// the one signal that actually discriminates, not an average.
///
/// That signal is the short-word ratio, which [`NativeTextStats`] already computes. On the
/// same document prose ran 0.04-0.28 and the drawings 0.42-0.47.
///
/// The cost here is asymmetric — a false positive deletes real text, a false negative only
/// leaves noise in place — so this is deliberately conservative: it declines to judge pages
/// with too few words to make the ratio meaningful, and its threshold sits in the middle of
/// the measured gap rather than at either edge.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn is_ocr_recognition_noise(text: &str, thresholds: &OcrQualityThresholds) -> bool {
    let stats = ocr_output_stats(text, thresholds);
    if stats.word_count < thresholds.min_words_for_ocr_output_check {
        return false;
    }
    stats.fragmented_word_ratio >= thresholds.max_ocr_output_fragmented_word_ratio
}

/// Accept a page's OCR text, or drop it and record why.
///
/// Returns the empty string for a rejected page. A rejected page must not fall back to
/// anything: the alternative to noise here is nothing, and a drawing that produced no
/// readable words genuinely has none. The warning is the only trace, so it always fires —
/// a page silently losing its text would be worse than the noise it replaces.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn accept_or_reject_ocr_page(
    page_index: usize,
    content: String,
    thresholds: &OcrQualityThresholds,
    warnings: &mut Vec<crate::types::ProcessingWarning>,
) -> String {
    if !is_ocr_recognition_noise(&content, thresholds) {
        return content;
    }
    let stats = ocr_output_stats(&content, thresholds);
    tracing::warn!(
        page = page_index + 1,
        words = stats.word_count,
        fragmented_word_ratio = stats.fragmented_word_ratio,
        threshold = thresholds.max_ocr_output_fragmented_word_ratio,
        "rejecting OCR output as recognition noise; the page contributes no text"
    );
    warnings.push(crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("ocr"),
        message: std::borrow::Cow::Owned(format!(
            "Page {} produced OCR output that is recognition noise rather than text \
             ({:.0}% of {} words are 1-2 characters, threshold {:.0}%); the page is most \
             likely a drawing or diagram. Its text was discarded.",
            page_index + 1,
            stats.fragmented_word_ratio * 100.0,
            stats.word_count,
            thresholds.max_ocr_output_fragmented_word_ratio * 100.0
        )),
    });
    String::new()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn compute_quality_score(text: &str, thresholds: &OcrQualityThresholds) -> f64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }

    // Score the prose content, not the Markdown scaffolding (#1341). Fall back to the
    // raw text if normalization leaves nothing (e.g. a table-only fragment).
    let normalized = normalize_markdown_for_scoring(trimmed);
    let scoring_input = if normalized.trim().is_empty() {
        trimmed
    } else {
        normalized.as_str()
    };

    let stats = NativeTextStats::compute(scoring_input, thresholds);

    let alnum_score = stats.alnum_ratio.min(1.0);
    let fragmentation_score = 1.0 - stats.fragmented_word_ratio.min(1.0);
    let word_length_score = (stats.avg_word_length / 5.0).min(1.0);
    let repeat_score = if thresholds.min_consecutive_repeat_ratio > 0.0 {
        1.0 - (stats.consecutive_repeat_ratio / thresholds.min_consecutive_repeat_ratio).min(1.0)
    } else {
        1.0
    };
    let meaningful_score = if thresholds.min_meaningful_words == 0 {
        1.0
    } else {
        (stats.meaningful_words as f64 / thresholds.min_meaningful_words as f64).min(1.0)
    };
    let garbage_score = if stats.garbage_char_count == 0 {
        1.0
    } else if thresholds.min_garbage_chars == 0 {
        0.0
    } else {
        (1.0 - stats.garbage_char_count as f64 / (thresholds.min_garbage_chars as f64 * 2.0)).max(0.0)
    };

    (alnum_score * 0.25
        + fragmentation_score * 0.20
        + word_length_score * 0.15
        + repeat_score * 0.15
        + meaningful_score * 0.15
        + garbage_score * 0.10)
        .clamp(0.0, 1.0)
}

/// Blend a pipeline stage's text-shape score with its reported confidence, when that
/// confidence is on a known legibility scale.
///
/// `mean_conf`, if `Some`, is already normalized to 0-1 by `extract_with_ocr` — and it is
/// only ever `Some` there when the reporting backend is `ConfidenceSemantics::Legibility`.
/// For any other backend `extract_with_ocr` reports `None`, so this drops the confidence
/// term entirely and blends nothing, rather than averaging in a number with no defined
/// scale (see `resolve_confidence_semantics`).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn pipeline_stage_score(text_score: f64, mean_conf: Option<f64>) -> f64 {
    match mean_conf {
        Some(conf) => text_score * 0.7 + conf * 0.3,
        None => text_score,
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn evaluate_per_page_ocr(
    native_text: &str,
    boundaries: Option<&[crate::types::PageBoundary]>,
    page_count: Option<u32>,
    thresholds: &OcrQualityThresholds,
) -> OcrFallbackDecision {
    let boundaries = match boundaries {
        Some(b) if !b.is_empty() => b,
        _ => return evaluate_native_text_for_ocr(native_text, page_count, thresholds),
    };

    let mut document_decision = evaluate_native_text_for_ocr(native_text, page_count, thresholds);

    if document_decision.whole_doc_failure {
        return document_decision;
    }

    let mut failing_pages: Vec<u32> = Vec::with_capacity(boundaries.len());
    let mut valid_boundary_count: usize = 0;
    for boundary in boundaries {
        if boundary.byte_start > boundary.byte_end
            || !native_text.is_char_boundary(boundary.byte_start)
            || !native_text.is_char_boundary(boundary.byte_end)
        {
            tracing::warn!(
                page = boundary.page_number,
                byte_start = boundary.byte_start,
                byte_end = boundary.byte_end,
                "skipping OCR quality evaluation for page with invalid text boundary"
            );
            continue;
        }
        valid_boundary_count += 1;
        let page_text = &native_text[boundary.byte_start..boundary.byte_end];
        if evaluate_native_text_for_ocr(page_text, Some(1), thresholds).fallback {
            failing_pages.push(boundary.page_number);
        }
    }

    if !failing_pages.is_empty() {
        document_decision.fallback = true;
        if failing_pages.len() == valid_boundary_count {
            document_decision.whole_doc_failure = true;
        }
    }
    document_decision.failing_pages = failing_pages;
    document_decision
}

/// Render only specific PDF pages to images for OCR processing.
///
/// `page_indices` are 0-indexed. Only the requested pages are rendered,
/// returned as `(page_index, image)` pairs.
// Gated to `ocr` rather than `any(ocr, ocr-pipeline)` to match its only
// callers in the `#[cfg(all(test, feature = "ocr"))]` test module. ~keep
#[cfg(all(test, feature = "ocr", feature = "pdf"))]
pub(crate) fn render_selected_pages_for_ocr(
    content: &[u8],
    page_indices: &[usize],
) -> crate::Result<Vec<(usize, image::DynamicImage)>> {
    let (doc, page_count, page_rotations) = open_pdf_for_page_ocr(content)?;
    let valid_indices = valid_page_indices(page_indices, page_count);
    render_selected_pages_from_document(&doc, &page_rotations, &valid_indices)
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn open_pdf_for_page_ocr(content: &[u8]) -> crate::Result<(pdf_oxide::PdfDocument, usize, Vec<u32>)> {
    let doc = pdf_oxide::PdfDocument::from_bytes(content.to_vec()).map_err(|e| crate::XbergError::Parsing {
        message: format!("Failed to open PDF for rendering: {}", e),
        source: None,
    })?;

    let page_count = doc.page_count().map_err(|e| crate::XbergError::Parsing {
        message: format!("Failed to get PDF page count: {}", e),
        source: None,
    })?;

    let page_rotations = crate::pdf::render::get_page_rotations(&doc, page_count);
    Ok((doc, page_count, page_rotations))
}

/// Page MediaBox size in points, falling back to US Letter (612x792pt) when the
/// PDF omits a MediaBox or it cannot be read.
///
/// Mirrors `crate::pdf::render`'s private page-dimension lookup; duplicated here
/// (rather than made `pub(crate)` there) because that module builds DPI-safeguard
/// logic on top of it that has no bearing on this file, and this needs only the
/// two-line MediaBox read to convert OCR pixel bboxes back into the PDF page's own
/// coordinate space (#1423).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn page_dimensions_pt(doc: &pdf_oxide::PdfDocument, page_index: usize) -> (f32, f32) {
    doc.get_page_media_box(page_index)
        .map(|(llx, lly, urx, ury)| ((urx - llx).abs(), (ury - lly).abs()))
        .unwrap_or((612.0, 792.0))
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn open_pdf_for_full_ocr(content: &[u8]) -> crate::Result<(pdf_oxide::PdfDocument, usize, Vec<u32>)> {
    let doc = pdf_oxide::PdfDocument::from_bytes(content.to_vec()).map_err(|e| crate::XbergError::Parsing {
        message: format!("Failed to open PDF for OCR streaming: {:?}", e),
        source: None,
    })?;
    let page_count = doc.page_count().map_err(|e| crate::XbergError::Parsing {
        message: format!("Failed to get document page count: {:?}", e),
        source: None,
    })?;
    let page_rotations = crate::pdf::render::get_page_rotations(&doc, page_count);
    Ok((doc, page_count, page_rotations))
}

/// Luma value at or below which a sampled pixel counts as ink.
///
/// Mid-gray, matching the `< 128` threshold the render-path glyph-ink assertions use
/// (`crate::pdf::render`'s `dark_pixels_in_cell`).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
const INK_LUMA_THRESHOLD: u8 = 128;

/// Sample every Nth pixel on both axes when probing a page raster for ink.
///
/// A blank-substituted page raster is uniformly white, so any subsample detects it;
/// 4 keeps the probe at 1/16 of the pixels (≈131k samples for a 150-DPI Letter page).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
const INK_PROBE_STRIDE: u32 = 4;

/// Fraction of sampled pixels that must be ink for the raster to count as non-blank.
///
/// 0.01% of a 150-DPI Letter page's subsample is ~13 pixels — below a single glyph's
/// ink, but far above the zero a blank-substituted raster yields.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
const INK_BLANK_MAX_DARK_RATIO: f64 = 0.0001;

/// Longest OCR text (in non-whitespace characters) that still justifies paying for an
/// ink probe of the page raster.
///
/// The probe exists only to catch a backend that *describes* a blank page instead of
/// returning nothing ("The image is entirely blank."), which `is_page_text_blank`'s
/// 3-character floor reads as content. Such answers are a sentence or two; a page that
/// was genuinely transcribed runs far longer. Gating on length keeps the PNG decode off
/// the hot path for real pages.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
const MAX_INK_PROBE_TEXT_CHARS: usize = 200;

/// Whether the rendered page raster carries essentially no ink.
///
/// Issue #1444: when pdf_oxide cannot draw a page's image XObjects it substitutes a
/// blank white bitmap, and a chatty backend then answers with a *description* of that
/// blankness rather than empty text — which [`is_page_text_blank`] accepts as content,
/// suppressing the XObject fallback. Looking at the pixels the backend was actually
/// given settles the question independently of what it said.
///
/// Returns `false` when `png_bytes` cannot be decoded: an undecodable raster is not
/// evidence of blankness, and the caller must not escalate on a guess.
///
/// [`is_page_text_blank`]: crate::extraction::blank_detection::is_page_text_blank
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn page_raster_is_blank(png_bytes: &[u8]) -> bool {
    let Ok(decoded) = image::load_from_memory(png_bytes) else {
        tracing::debug!("ink probe: page raster could not be decoded; not treating it as blank");
        return false;
    };
    let luma = decoded.to_luma8();
    let (width, height) = luma.dimensions();
    if width == 0 || height == 0 {
        return true;
    }

    let mut sampled: u64 = 0;
    let mut dark: u64 = 0;
    for y in (0..height).step_by(INK_PROBE_STRIDE as usize) {
        for x in (0..width).step_by(INK_PROBE_STRIDE as usize) {
            sampled += 1;
            if luma.get_pixel(x, y).0[0] < INK_LUMA_THRESHOLD {
                dark += 1;
            }
        }
    }

    (dark as f64) <= (sampled as f64) * INK_BLANK_MAX_DARK_RATIO
}

/// Whether this page should be treated as blank for the purposes of the image-XObject
/// OCR fallback.
///
/// Blank by text (the pre-existing [`is_page_text_blank`] rule) **or** blank by ink: a
/// short OCR answer over a raster with no ink on it is a description of a blank page,
/// not a transcription of one. The text test is free and runs first; the ink probe is
/// additionally gated on [`MAX_INK_PROBE_TEXT_CHARS`] so a genuinely transcribed page
/// never pays for a PNG decode.
///
/// [`is_page_text_blank`]: crate::extraction::blank_detection::is_page_text_blank
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn page_needs_xobject_fallback(ocr_text: &str, page_png: &[u8]) -> bool {
    if crate::extraction::blank_detection::is_page_text_blank(ocr_text) {
        return true;
    }
    let non_whitespace = ocr_text.chars().filter(|c| !c.is_whitespace()).count();
    non_whitespace <= MAX_INK_PROBE_TEXT_CHARS && page_raster_is_blank(page_png)
}

/// What one page's image-XObject OCR recovery attempt produced.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
struct XObjectRecoveryOutcome {
    /// Concatenated OCR text of every embedded image that yielded any; empty when none did.
    text: String,
    /// How many image XObjects were handed to the backend.
    attempted: usize,
    /// The recovered images themselves, provenance-tagged for the output's `images` array.
    images: Vec<crate::types::ExtractedImage>,
}

/// OCR a page's embedded image XObjects directly, bypassing the whole-page rasterizer.
///
/// Used when the page render came back blank (see [`page_needs_xobject_fallback`]) but the
/// page does carry image XObjects the renderer could not paint (issue #1355/#1444).
///
/// Returns `None` when the page has no recoverable image XObjects at all, so the caller can
/// tell "nothing to try" apart from "tried and got nothing" and avoid warning about a page
/// that was simply empty.
///
/// Provenance: each recovered image is tagged `source_path = "xobject:page{N}:{i}"` (`N`
/// 1-based, `i` the image's 0-based paint order on that page) with the recovery mode in
/// `description`. This reuses the existing `source_path` convention (DOCX/ODT record
/// `media/imageN.png` there) rather than adding a field to `ExtractedImage`, which would
/// require regenerating every language binding.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
async fn recover_page_text_from_image_xobjects(
    backend: &std::sync::Arc<dyn crate::plugins::OcrBackend>,
    render_doc: &pdf_oxide::PdfDocument,
    page_idx: usize,
    ocr_config: &crate::core::config::OcrConfig,
) -> Option<XObjectRecoveryOutcome> {
    let fallback_images = crate::pdf::oxide::images::page_ocr_fallback_image_bytes(render_doc, page_idx);
    if fallback_images.is_empty() {
        return None;
    }

    let mut recovered = String::new();
    let mut provenance = Vec::with_capacity(fallback_images.len());
    for (image_index, fallback) in fallback_images.iter().enumerate() {
        match backend.process_image(&fallback.bytes, ocr_config).await {
            Ok(result) if !result.content.trim().is_empty() => {
                if !recovered.is_empty() {
                    recovered.push_str("\n\n");
                }
                recovered.push_str(&result.content);
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(
                    page = page_idx,
                    "force_ocr fallback: OCR of embedded image bytes failed: {error}"
                );
            }
        }
        provenance.push(crate::types::ExtractedImage {
            data: fallback.bytes.clone(),
            format: std::borrow::Cow::Borrowed(fallback.format),
            image_index: image_index as u32,
            page_number: Some((page_idx + 1) as u32),
            source_path: Some(format!("xobject:page{}:{}", page_idx + 1, image_index)),
            description: Some(format!(
                "recovered from raw image XObject ({}) after the page rasterizer produced a blank page",
                fallback.recovery.as_str()
            )),
            ..Default::default()
        });
    }

    Some(XObjectRecoveryOutcome {
        text: recovered,
        attempted: fallback_images.len(),
        images: provenance,
    })
}

/// The warning that makes an image-XObject recovery visible in the output.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn xobject_fallback_warning(page_idx: usize, attempted: usize) -> crate::types::ProcessingWarning {
    crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("ocr"),
        message: std::borrow::Cow::Owned(format!(
            "Page {} rendered blank but contains {} image XObject(s) the PDF rasterizer \
             could not draw; OCR was retried on the embedded image bytes.",
            page_idx + 1,
            attempted
        )),
    }
}

/// Lazily open — at most once — a PDF document used *only* by the image-XObject OCR
/// fallback.
///
/// The main `lazy_pdf_render_state` is deliberately not opened when the caller supplied
/// pre-rendered `images` (the layout-detection route), because its page-rotation and
/// points-per-pixel lookups are indexed differently there. The fallback needs nothing but
/// the page's XObject table, so it gets its own handle rather than perturbing those
/// lookups. Opening is deferred until a page actually comes back blank, so the common
/// case pays nothing.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn fallback_render_document<'a>(
    memo: &'a mut Option<Option<pdf_oxide::PdfDocument>>,
    content: Option<&[u8]>,
) -> Option<&'a pdf_oxide::PdfDocument> {
    memo.get_or_insert_with(|| {
        let bytes = content?;
        match open_pdf_for_full_ocr(bytes) {
            Ok((doc, _, _)) => Some(doc),
            Err(error) => {
                tracing::debug!("force_ocr fallback: reopening the PDF for XObject recovery failed: {error}");
                None
            }
        }
    })
    .as_ref()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn render_full_pdf_ocr_batch(
    doc: &pdf_oxide::PdfDocument,
    page_rotations: &[u32],
    page_range: std::ops::Range<usize>,
) -> crate::Result<Vec<EncodedPage>> {
    let mut encoded = Vec::with_capacity(page_range.len());
    for page_idx in page_range {
        let rendered = crate::pdf::render::render_page_with_safeguards(doc, page_idx, 150).map_err(|e| {
            crate::XbergError::Parsing {
                message: format!("Failed to render page {} for OCR: {:?}", page_idx, e),
                source: None,
            }
        })?;
        let rotation = page_rotations.get(page_idx).copied().unwrap_or(0);
        let (data, width, height) = crate::pdf::render::normalize_rendered_page_for_ocr(
            rendered.data,
            rendered.width,
            rendered.height,
            rotation,
        )?;
        encoded.push((page_idx, std::sync::Arc::new(data), width, height));
    }
    Ok(encoded)
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn valid_page_indices(page_indices: &[usize], page_count: usize) -> Vec<usize> {
    page_indices
        .iter()
        .copied()
        .filter(|&idx| {
            if idx < page_count {
                true
            } else {
                tracing::warn!(
                    page = idx + 1,
                    page_count,
                    "force_ocr_pages: page {} is out of range (document has {} pages), skipping",
                    idx + 1,
                    page_count
                );
                false
            }
        })
        .collect()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn render_selected_pages_from_document(
    doc: &pdf_oxide::PdfDocument,
    page_rotations: &[u32],
    page_indices: &[usize],
) -> crate::Result<Vec<(usize, image::DynamicImage)>> {
    let mut images = Vec::with_capacity(page_indices.len());
    for &idx in page_indices {
        let rendered =
            crate::pdf::render::render_page_with_safeguards(doc, idx, 150).map_err(|e| crate::XbergError::Parsing {
                message: format!("Failed to render PDF page {}: {}", idx + 1, e),
                source: None,
            })?;
        let rotation = page_rotations.get(idx).copied().unwrap_or(0);
        let (data, _, _) = crate::pdf::render::normalize_rendered_page_for_ocr(
            rendered.data,
            rendered.width,
            rendered.height,
            rotation,
        )?;
        let img = image::load_from_memory(&data).map_err(|e| crate::XbergError::Parsing {
            message: format!("Failed to decode rendered page {}: {}", idx + 1, e),
            source: None,
        })?;
        images.push((idx, img));
    }

    Ok(images)
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn share_rendered_page_images(
    page_images: Vec<(usize, image::DynamicImage)>,
) -> Vec<(usize, std::sync::Arc<image::DynamicImage>)> {
    page_images
        .into_iter()
        .map(|(page_idx, image)| (page_idx, std::sync::Arc::new(image)))
        .collect()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn normalize_mixed_ocr_document_page(doc: &mut crate::types::internal::InternalDocument, page_number: u32) {
    for element in &mut doc.elements {
        if !matches!(element.kind, crate::types::internal::ElementKind::PageBreak) {
            element.page = Some(page_number);
        }
    }
    for table in &mut doc.tables {
        table.page_number = page_number;
    }
    for image in &mut doc.images {
        image.page_number = Some(page_number);
    }
}

/// Height-axis points-per-pixel ratio for one page's raster, used to scale
/// `element.ocr_geometry`'s pixel-space quad-edge height into the font-size
/// resolver's PDF-points unit (see
/// [`crate::pdf::structure::adapters::OcrFontSizeScale`]). Falls back to a no-op
/// scale of `1.0` when there is no raster height to divide by, or the computed
/// ratio is not a finite positive number -- mirrors `ocr_points_per_pixel`'s same
/// guard, for the same reason: leave the pixel value unconverted rather than
/// fabricate a scale or divide by zero.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn mixed_route_geometry_points_per_pixel(page_height_pt: f32, image_height_px: u32) -> f32 {
    const NO_OP_POINTS_PER_PIXEL: f32 = 1.0;
    if image_height_px == 0 {
        return NO_OP_POINTS_PER_PIXEL;
    }
    let scale = page_height_pt / image_height_px as f32;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        NO_OP_POINTS_PER_PIXEL
    }
}

/// Returns the assembled per-page document alongside the bare, unclassified paragraphs
/// used to build it -- the latter feeds [`extract_mixed_ocr_native`]'s document-global
/// heading/list heuristic pass, which needs every OCR'd page's paragraphs in hand at
/// once (see that function's own comments, and `extract_with_ocr_for_page`'s doc comment
/// on `skip_document_global_heuristic` for why a single page can't run this heuristic
/// itself).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn assemble_mixed_ocr_page_document(
    mut doc: crate::types::internal::InternalDocument,
    page_number: u32,
    page_height: u32,
    geometry_points_per_pixel: f32,
) -> (
    crate::types::internal::InternalDocument,
    Vec<crate::pdf::structure::types::PdfParagraph>,
) {
    // `doc.elements[].bbox` is already in PDF points by the time this runs (the caller
    // rescales it via `rescale_ocr_bboxes_to_page_points` before calling this), and
    // `page_height` is the page's own height in points, so the bbox-height font-size
    // fallback needs no further scaling. `doc.elements[].ocr_geometry`, in contrast, is
    // NOT touched by that rescale -- it stays raw OCR raster pixels (see
    // `extraction::derive::OcrElement::geometry`'s documented raster-pixel-space
    // contract) -- so the quad-edge fallback (sceptre/paddle) still needs the real
    // points-per-pixel ratio for this page, `geometry_points_per_pixel`. See
    // `pdf::structure::adapters::OcrFontSizeScale` for why these can't share one scalar.
    let font_size_scale =
        crate::pdf::structure::adapters::OcrFontSizeScale::bbox_already_in_points(geometry_points_per_pixel);
    let paragraphs = crate::pdf::structure::adapters::ocr_doc_to_paragraphs(&doc, page_height, font_size_scale);
    if !paragraphs.is_empty() {
        let mut assembled = crate::pdf::structure::assemble_internal_document(
            vec![paragraphs.clone()],
            &doc.tables,
            Some(&doc.images),
            &[],
        );
        assembled.processing_warnings = std::mem::take(&mut doc.processing_warnings);
        doc = assembled;
    }

    normalize_mixed_ocr_document_page(&mut doc, page_number);
    (doc, paragraphs)
}

/// Flat OCR-text document for a page whose backend produced tables or OCR elements
/// but no structured document.
///
/// Mirrors the paragraph shape of the raw-text fallback in `append_ocr_replacements`
/// so the page reads identically, while giving its assets a document to travel in.
///
/// OCR page text is normalized to LF first: backend output is not uniformly LF-only.
/// Tesseract emits LF, but the VLM backend (`crate::llm::vlm_ocr`) returns the model's
/// markdown verbatim out of an HTTP JSON body, which routinely carries `\r\n`. Splitting
/// raw would fold the entire page into a single block element (#316).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn flat_ocr_page_document(text: &str) -> crate::types::internal::InternalDocument {
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
    use crate::types::ocr_elements::OcrElementLevel;

    let mut doc = InternalDocument::new("pdf");
    let text = crate::extraction::transform::normalize_line_endings(text);
    for paragraph in text
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
    {
        doc.push_element(InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            paragraph,
            0,
        ));
    }
    doc
}

/// Attach a page's OCR tables and OCR elements to its structured document.
///
/// The mixed route used to discard both (#60): only `ocr_internal_document` was kept,
/// so tables recognised on an OCR'd page and every word-level bounding box were lost.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn attach_page_ocr_payload(
    doc: &mut crate::types::internal::InternalDocument,
    tables: Vec<crate::types::Table>,
    elements: Vec<crate::types::OcrElement>,
    page_number: u32,
) {
    if doc.tables.is_empty() {
        doc.tables = tables;
    }
    if !elements.is_empty() {
        let mut elements = elements;
        for element in &mut elements {
            element.page_number = page_number;
        }
        doc.prebuilt_ocr_elements.get_or_insert_with(Vec::new).extend(elements);
    }
}

/// Rescale an OCR backend's pixel-space bounding boxes into the PDF page's own
/// coordinate space before its structured document is assembled (#1423).
///
/// On non-OCR pages, `document.nodes[].bbox`, `pages[].hierarchy.blocks[].bbox`, and
/// `chunks[].metadata.page_spans[].bbox` are all in PDF points with a bottom-left
/// origin. On OCR'd pages they previously stayed in raw Tesseract raster pixels
/// (top-left origin), with no field anywhere reporting the raster size needed to
/// convert them back.
///
/// `element` bboxes (word/line/block boxes from the OCR document) are only scaled
/// from pixels to points here, still top-left; `ocr_doc_to_paragraphs`
/// (`crate::pdf::structure::adapters::pdf_block_bbox`) performs the top-left ->
/// bottom-left flip further down the pipeline using the page height passed to
/// [`assemble_mixed_ocr_page_document`] — which must therefore be in points, not
/// raster pixels, from this point on.
///
/// `table` bounding boxes are copied through unchanged by every later step (no flip
/// is applied to them anywhere else in the pipeline), so this function performs the
/// full pixel-to-point conversion *and* the y-flip for those directly, matching the
/// bottom-left/points contract documented on [`crate::types::Table::bounding_box`].
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn rescale_ocr_bboxes_to_page_points(
    doc: Option<&mut crate::types::internal::InternalDocument>,
    tables: &mut [crate::types::Table],
    image_width_px: u32,
    image_height_px: u32,
    page_width_pt: f32,
    page_height_pt: f32,
) {
    if image_width_px == 0 || image_height_px == 0 {
        // No raster dimensions to convert from (e.g. a synthetic/test document with
        // no rendered page behind it) — leave bboxes as-is rather than dividing by
        // zero or fabricating a scale factor.
        return;
    }
    let scale_x = f64::from(page_width_pt) / f64::from(image_width_px);
    let scale_y = f64::from(page_height_pt) / f64::from(image_height_px);

    if let Some(doc) = doc {
        for element in &mut doc.elements {
            if let Some(bbox) = element.bbox.as_mut() {
                bbox.x0 *= scale_x;
                bbox.x1 *= scale_x;
                bbox.y0 *= scale_y;
                bbox.y1 *= scale_y;
            }
        }
    }

    let page_height_pt_f64 = f64::from(page_height_pt);
    for table in tables.iter_mut() {
        if let Some(bbox) = table.bounding_box.as_mut() {
            // `convert_ocr_table` (crates/xberg/src/ocr/tesseract_backend.rs) stores the
            // raw pixel rect verbatim as {x0: left, y0: top, x1: right, y1: bottom} —
            // top-left origin, unscaled pixels. Convert and flip in one step.
            let (left_px, top_px, right_px, bottom_px) = (bbox.x0, bbox.y0, bbox.x1, bbox.y1);
            bbox.x0 = left_px * scale_x;
            bbox.x1 = right_px * scale_x;
            bbox.y0 = page_height_pt_f64 - bottom_px * scale_y;
            bbox.y1 = page_height_pt_f64 - top_px * scale_y;
        }
    }
}

/// Undo a single quarter-turn on one point, mapping a backend's post-auto-rotate
/// pixel space back to the pre-rotation raster it was actually given.
///
/// `processed_width`/`processed_height` are the dimensions of the space `(x, y)`
/// is currently in (i.e. the auto-rotated image the backend ran detection on).
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn undo_auto_rotate_point(
    x: f64,
    y: f64,
    correction_degrees: u16,
    processed_width: f64,
    processed_height: f64,
) -> (f64, f64) {
    match correction_degrees {
        90 => (y, processed_height - x),
        180 => (processed_width - x, processed_height - y),
        270 => (processed_width - y, x),
        _ => (x, y),
    }
}

/// Undo an OCR backend's `auto_rotate` orientation correction on a structured
/// document's element bboxes, mapping them back from the rotated image the
/// backend actually OCR'd to the raster its caller rendered and will rescale
/// (#633).
///
/// Some backends (currently PaddleOCR, see
/// `paddle_ocr::backend::rotate_for_detected_orientation`) detect a scanned
/// page's orientation and rotate their input image before OCR when
/// `OcrConfig::auto_rotate` is set, recording that in the result metadata
/// (`ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY` /
/// `OCR_ORIENTATION_DEGREES_METADATA_KEY` /
/// `OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY` / `..._HEIGHT_...`). Their
/// `ocr_internal_document` bboxes are built directly from that rotated raster
/// and are never mapped back — every other caller of this document, including
/// `rescale_ocr_bboxes_to_page_points` below, assumes bboxes are in the
/// *original* `render_width`/`render_height` raster (the one the caller
/// rendered and passed to the backend), which after a 90/270 correction has
/// different — swapped — dimensions than the rotated one the bboxes are
/// actually in. Left uncorrected, the pixel->point rescale divides by the wrong
/// axis and both position and reading order come out wrong.
///
/// A no-op when the backend didn't auto-rotate (the metadata key is absent),
/// which covers every backend and the overwhelmingly common `auto_rotate: false`
/// default.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn undo_auto_rotate_document_bboxes(
    doc: &mut crate::types::internal::InternalDocument,
    metadata: &crate::types::Metadata,
    render_width: u32,
    render_height: u32,
) {
    let auto_rotated = metadata
        .additional
        .get(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !auto_rotated {
        return;
    }
    let Some(orientation) = metadata
        .additional
        .get(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY)
        .and_then(serde_json::Value::as_i64)
    else {
        return;
    };
    if !matches!(orientation, 0 | 90 | 180 | 270) {
        return;
    }
    let correction_degrees = ((360 - orientation).rem_euclid(360)) as u16;
    if correction_degrees == 0 {
        return;
    }
    // Prefer the backend's own reported processed-image size; fall back to the
    // swap a lossless quarter-turn of the original raster implies, in case a
    // future backend sets `auto_rotated` without the paired width/height keys.
    let reported_dimensions = metadata
        .additional
        .get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY)
        .and_then(serde_json::Value::as_u64)
        .zip(
            metadata
                .additional
                .get(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY)
                .and_then(serde_json::Value::as_u64),
        );
    let (processed_width, processed_height) = match reported_dimensions {
        Some((width, height)) => (width as f64, height as f64),
        None if matches!(correction_degrees, 90 | 270) => (f64::from(render_height), f64::from(render_width)),
        None => (f64::from(render_width), f64::from(render_height)),
    };
    for element in &mut doc.elements {
        let Some(bbox) = element.bbox.as_mut() else {
            continue;
        };
        let (x0, y0) = undo_auto_rotate_point(bbox.x0, bbox.y0, correction_degrees, processed_width, processed_height);
        let (x1, y1) = undo_auto_rotate_point(bbox.x1, bbox.y1, correction_degrees, processed_width, processed_height);
        bbox.x0 = x0.min(x1);
        bbox.x1 = x0.max(x1);
        bbox.y0 = y0.min(y1);
        bbox.y1 = y0.max(y1);
    }
}

/// Build the per-page structured document for the single-backend mixed OCR route,
/// carrying the backend's tables and OCR elements instead of dropping them (#60).
///
/// Returns `None` only when the backend produced nothing structured at all, which
/// keeps the raw-text replacement path unchanged for plain-text pages.
///
/// `image_width_px`/`image_height_px` are the rendered page raster's pixel
/// dimensions and `page_width_pt`/`page_height_pt` are the PDF page's own MediaBox
/// size in points; together they let every OCR bbox be rescaled into the page's
/// coordinate space before assembly (#1423).
///
/// Returns the assembled document alongside the bare, unclassified paragraphs
/// [`assemble_mixed_ocr_page_document`] built it from -- see that function's doc
/// comment.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn build_mixed_ocr_page_document(
    result: &mut crate::types::ExtractedDocument,
    page_number: u32,
    image_width_px: u32,
    image_height_px: u32,
    page_width_pt: f32,
    page_height_pt: f32,
) -> Option<(
    crate::types::internal::InternalDocument,
    Vec<crate::pdf::structure::types::PdfParagraph>,
)> {
    let mut backend_tables = std::mem::take(&mut result.tables);
    let backend_elements = result.ocr_elements.take().unwrap_or_default();
    let mut doc = match result.ocr_internal_document.take() {
        Some(doc) => doc,
        None if backend_tables.is_empty() && backend_elements.is_empty() => return None,
        None => flat_ocr_page_document(&result.content),
    };
    undo_auto_rotate_document_bboxes(&mut doc, &result.metadata, image_width_px, image_height_px);
    rescale_ocr_bboxes_to_page_points(
        Some(&mut doc),
        &mut backend_tables,
        image_width_px,
        image_height_px,
        page_width_pt,
        page_height_pt,
    );
    attach_page_ocr_payload(&mut doc, backend_tables, Vec::new(), page_number);
    // `assemble_mixed_ocr_page_document`/`ocr_doc_to_paragraphs` still take the page
    // nearest point loses at most ~0.5pt, negligible next to the pixel-vs-point unit
    // bug this rescale fixes.
    let page_height_rounded_pt = page_height_pt.max(0.0).round() as u32;
    let geometry_points_per_pixel = mixed_route_geometry_points_per_pixel(page_height_pt, image_height_px);
    let (mut assembled, paragraphs) =
        assemble_mixed_ocr_page_document(doc, page_number, page_height_rounded_pt, geometry_points_per_pixel);
    attach_page_ocr_payload(&mut assembled, Vec::new(), backend_elements, page_number);
    Some((assembled, paragraphs))
}

/// Convert one OCR formula bbox to PDF points.
///
/// Backends can rescale the page image before OCR; when the result metadata
/// carries the processed dimensions, those describe the bbox's pixel space
/// and take precedence over the rendered dimensions.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn formula_bbox_to_page_points(
    formula: &mut crate::types::Formula,
    doc: &pdf_oxide::PdfDocument,
    page_idx: usize,
    metadata: Option<&crate::types::Metadata>,
    rendered_w: u32,
    rendered_h: u32,
) {
    if let Some(bbox) = formula.bbox {
        let (px_w, px_h) = metadata
            .and_then(processed_ocr_layout_dimensions)
            .unwrap_or((rendered_w, rendered_h));
        let (w_pt, h_pt) = crate::pdf::render::get_page_dimensions_pt(doc, page_idx);
        formula.bbox = Some(crate::pdf::render::pixel_bbox_to_pdf_points(
            bbox, px_w, px_h, w_pt, h_pt,
        ));
    }
}

/// Flip the bboxes of a document's table elements from a top-left to a bottom-left
/// origin, in points.
///
/// `crate::pdf::structure::assembly::push_table_element` copies `Table::bounding_box`
/// verbatim onto the table's element, so on the pipeline route that element inherits the
/// table's raw top-left pixel rect while every paragraph element around it was already
/// flipped (in pixel space) by `ocr_doc_to_paragraphs`. Once
/// [`rescale_ocr_bboxes_to_page_points`] has put both in points, only the table elements
/// still need the flip the single-backend route gives them before assembly.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn flip_table_element_bboxes_to_bottom_left(doc: &mut crate::types::internal::InternalDocument, page_height_pt: f32) {
    let page_height_pt = f64::from(page_height_pt);
    for element in &mut doc.elements {
        if matches!(element.kind, crate::types::internal::ElementKind::Table { .. })
            && let Some(bbox) = element.bbox.as_mut()
        {
            let (top, bottom) = (bbox.y0, bbox.y1);
            bbox.y0 = page_height_pt - bottom;
            bbox.y1 = page_height_pt - top;
        }
    }
}

/// Build the per-page structured document for the multi-stage pipeline / `vlm_fallback`
/// route, converting its pixel-space bboxes into the PDF page's point space (#1423).
///
/// The single-backend route's [`build_mixed_ocr_page_document`] cannot be reused as a
/// shared choke point: it takes the backend's *raw* OCR document and rescales it before
/// running assembly, whereas `run_ocr_pipeline` returns a document `extract_with_ocr` has
/// already assembled — its element bboxes carry the top-left -> bottom-left flip applied
/// with the *raster's* pixel height, so re-assembling it here would flip them a second
/// time. Only the pixel -> point scale is missing, which is exactly what
/// [`rescale_ocr_bboxes_to_page_points`] applies to document elements (tables, whose
/// bboxes are raw top-left pixel rects on this route too, still get the full
/// scale-and-flip).
///
/// `raster_size_px` is the rendered page image this route OCR'd; `page_size_pt` is the
/// page's own MediaBox size in points.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn build_pipeline_ocr_page_document(
    doc: Option<crate::types::internal::InternalDocument>,
    mut tables: Vec<crate::types::Table>,
    elements: Vec<crate::types::OcrElement>,
    page_text: &str,
    page_number: u32,
    raster_size_px: (u32, u32),
    page_size_pt: (f32, f32),
) -> Option<crate::types::internal::InternalDocument> {
    if doc.is_none() && tables.is_empty() && elements.is_empty() {
        return None;
    }
    let mut doc = doc.unwrap_or_else(|| flat_ocr_page_document(page_text));
    let (raster_width_px, raster_height_px) = raster_size_px;
    let (page_width_pt, page_height_pt) = page_size_pt;

    // Tables already folded into the assembled document are a separate allocation from
    // the `tables` returned alongside it, so each is converted exactly once.
    let mut assembled_tables = std::mem::take(&mut doc.tables);
    rescale_ocr_bboxes_to_page_points(
        Some(&mut doc),
        &mut assembled_tables,
        raster_width_px,
        raster_height_px,
        page_width_pt,
        page_height_pt,
    );
    if raster_width_px != 0 && raster_height_px != 0 {
        flip_table_element_bboxes_to_bottom_left(&mut doc, page_height_pt);
    }
    doc.tables = assembled_tables;
    rescale_ocr_bboxes_to_page_points(
        None,
        &mut tables,
        raster_width_px,
        raster_height_px,
        page_width_pt,
        page_height_pt,
    );

    attach_page_ocr_payload(&mut doc, tables, elements, page_number);
    normalize_mixed_ocr_document_page(&mut doc, page_number);
    Some(doc)
}

/// Wraps a single OCR backend in a one-stage [`crate::core::config::OcrPipelineConfig`] so
/// [`extract_mixed_ocr_native`] can route it through [`run_ocr_pipeline_for_page`] --
/// the only per-page entry point that threads `layout_detections` down into
/// [`extract_with_ocr_for_page`]'s pixel-space layout classification -- instead of this
/// route's own raw `backend.process_image_owned` fast path, which never accepted layout
/// detections at all (#665: `--layout` alone produced byte-identical mixed-route output).
/// Mirrors the `classical_stage` construction in [`crate::core::config::ocr::OcrConfig::effective_pipeline`].
///
/// Only used when this call actually has layout detections to offer (see
/// `layout_detections_for_mixed` in [`extract_mixed_ocr_native`]); the plain single-backend
/// fast path is untouched when layout is off, so non-layout mixed-route output stays
/// byte-identical.
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    feature = "pdf",
    feature = "layout-detection"
))]
fn single_stage_pipeline_for_layout(
    ocr_config: &crate::core::config::OcrConfig,
) -> crate::core::config::OcrPipelineConfig {
    crate::core::config::OcrPipelineConfig {
        stages: vec![crate::core::config::OcrPipelineStage {
            backend: ocr_config.backend.clone(),
            priority: 100,
            language: if ocr_config.language.len() == 1 && ocr_config.language[0] == "eng" {
                None
            } else {
                Some(ocr_config.language.clone())
            },
            tesseract_config: ocr_config.tesseract_config.clone(),
            paddle_ocr_config: None,
            vlm_config: None,
            backend_options: ocr_config.backend_options.clone(),
        }],
        quality_thresholds: ocr_config.effective_thresholds(),
    }
}

/// Looks up one document-wide 0-based page's own layout detection out of the whole-document
/// pass [`extract_mixed_ocr_native`] runs (#665).
///
/// Performs no coordinate transform: `detections` is exactly what
/// `layout_runner::run_layout_for_ocr` produced (pixel space, at the resolution its own
/// per-page render used), and this function returns that same value unchanged for whichever
/// page it belongs to. The rescale to this page's *own* OCR raster
/// (`scale_detection_to_dimensions` / `scale_detection_to_ocr_coordinates`) and, later, to PDF
/// points (`rescale_ocr_bboxes_to_page_points` inside `build_pipeline_ocr_page_document`) both
/// happen downstream, inside `extract_with_ocr_for_page` -- not here. Keeping this lookup a
/// pure index (rather than folding a rescale into it) means a page-alignment bug here shows up
/// as a wrong page's detection landing on the wrong page, not as a subtly-wrong coordinate on
/// the right one.
#[cfg(all(
    any(feature = "ocr", feature = "ocr-pipeline"),
    feature = "pdf",
    feature = "layout-detection"
))]
fn detection_for_mixed_route_page(
    detections: Option<&[crate::layout::DetectionResult]>,
    page_idx: usize,
) -> Option<&crate::layout::DetectionResult> {
    detections.and_then(|detections| detections.get(page_idx))
}

/// Build mixed text from native extraction and per-page OCR results.
///
/// For each page boundary, if the page is in `ocr_page_numbers` (1-indexed),
/// use the OCR result; otherwise use the native text slice.
///
/// Page numbers must be >= 1 (invalid values are filtered out with a warning).
/// An `ocr` config is recommended but not required; defaults are used if absent.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
pub(crate) async fn extract_mixed_ocr_native(
    native_text: &str,
    boundaries: &[crate::types::PageBoundary],
    ocr_page_numbers: &[u32],
    content: &[u8],
    config: &ExtractionConfig,
    _path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    ahash::AHashMap<u32, String>,
    ahash::AHashMap<u32, crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    Vec<crate::types::ProcessingWarning>,
)> {
    let ocr_set: std::collections::HashSet<u32> = ocr_page_numbers
        .iter()
        .copied()
        .filter(|&p| {
            if p == 0 {
                tracing::warn!("force_ocr_pages contains 0; page numbers are 1-indexed, ignoring");
                false
            } else {
                true
            }
        })
        .collect();

    if ocr_set.is_empty() {
        return Ok((
            native_text.to_string(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        ));
    }

    let mut page_indices: Vec<usize> = ocr_set.iter().map(|&p| (p - 1) as usize).collect();
    page_indices.sort_unstable();
    let (render_doc, page_count, page_rotations) = open_pdf_for_page_ocr(content)?;
    page_indices = valid_page_indices(&page_indices, page_count);
    if page_indices.is_empty() {
        return Ok((
            native_text.to_string(),
            ahash::AHashMap::new(),
            ahash::AHashMap::new(),
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
        ));
    }

    // Layout detection for this mixed OCR route (#665). The full-document OCR routes
    // (`force_ocr`, the OCR-gate fallback) already run layout via `run_ocr_with_layout` ->
    // `layout_runner::run_layout_for_ocr`, keyed on `config.resolved_layout_config()` (i.e.
    // `config.layout` being set, which `--layout` alone does). This route never called that:
    // it built `structured_ocr_pages` straight from raw backend OCR output, so `--layout`
    // alone produced byte-identical text with zero layout log lines even though layout
    // detection is what should be classifying headings/lists/tables here. Runs the exact
    // same whole-document pass (`RenderWithoutInference`: every page renders, gated pages
    // skip inference, CPU-retry-on-accelerated-failure) the full-document routes use; only
    // the pages this call actually OCRs read from the result below. `page_idx` throughout
    // this function is the same document-wide 0-based index `run_layout_for_ocr`'s per-page
    // `Vec` is indexed by, so `detections.get(page_idx)` needs no extra alignment step.
    #[cfg(feature = "layout-detection")]
    let (layout_detections_for_mixed, layout_pass_warning, layout_pass_glyph_drop_warnings): (
        Option<Vec<crate::layout::DetectionResult>>,
        Option<crate::types::ProcessingWarning>,
        Vec<crate::types::ProcessingWarning>,
    ) = if let Some(layout_config) = config.resolved_layout_config() {
        let layout_thread_budget = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());
        match super::layout_runner::run_layout_for_ocr(content, layout_config.as_ref(), layout_thread_budget).await {
            Ok((
                super::layout_runner::LayoutAttempt {
                    output:
                        super::layout_runner::LayoutRunOutput {
                            data: Some((_, _, _, detections)),
                            ..
                        },
                    warning,
                    ..
                },
                glyph_drop_warnings,
            )) => (Some(detections), warning, glyph_drop_warnings),
            Ok((
                super::layout_runner::LayoutAttempt {
                    output: super::layout_runner::LayoutRunOutput { data: None, .. },
                    warning,
                    ..
                },
                glyph_drop_warnings,
            )) => {
                tracing::info!(
                    "OCR layout (mixed route): auto gate skipped every page, continuing without layout assembly"
                );
                (None, warning, glyph_drop_warnings)
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "OCR layout detection failed for mixed OCR route; continuing without layout assembly"
                );
                (
                    None,
                    Some(super::layout_runner::layout_failure_warning(&error)),
                    Vec::new(),
                )
            }
        }
    } else {
        (None, None, Vec::new())
    };
    #[cfg(feature = "layout-detection")]
    let mixed_route_layout_active = layout_detections_for_mixed.is_some();
    #[cfg(not(feature = "layout-detection"))]
    let mixed_route_layout_active = false;

    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    // rayon's work-stealing pool needs OS threads; wasm32 has none, so the parallel encode
    // paths below fall back to sequential `.iter()` there. Gate the import to match. ~keep
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use rayon::prelude::*;
    use std::io::Cursor;
    use std::sync::Arc;

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let mut ocr_config_resolved = config.ocr.as_ref().unwrap_or(&default_ocr_config).clone();
    if ocr_config_resolved.acceleration.is_none() {
        ocr_config_resolved.acceleration = config.acceleration.clone();
    }

    let batch_size = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());

    let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
    let ocr_config_owned = ensure_elements_enabled(&ocr_config_resolved);
    // When a `vlm_fallback` policy or an explicit multi-stage `pipeline` is configured,
    // each page must run through the shared pipeline runner so fallback backends (e.g.
    // the VLM) apply on this mixed/per-page OCR route too. Previously only the single
    // configured backend ran here, silently ignoring `vlm_fallback` on the
    // `scanned_pages` / `force_ocr_pages` / per-page-fallback routes (#1341). The
    // default (no fallback, no explicit pipeline) keeps the fast single-backend path.
    //
    // Layout detections (#665, `mixed_route_layout_active`) are threaded the same way: the
    // pipeline route is the only one that hands `layout_detections` down to
    // `extract_with_ocr_for_page`'s classification. Wrapping the single configured backend
    // in a one-stage pipeline (`single_stage_pipeline_for_layout`) reuses that exact,
    // already-tested code path instead of duplicating pixel-space layout assembly here. This
    // only fires when a real detection is available for this call, so `--layout` producing
    // nothing (gate skipped every page, or no `config.layout`) leaves the fast path untouched.
    let effective_pipeline = if ocr_config_owned.vlm_fallback != crate::core::config::VlmFallbackPolicy::Disabled
        || ocr_config_owned.pipeline.is_some()
    {
        ocr_config_owned.effective_pipeline()
    } else if mixed_route_layout_active {
        #[cfg(feature = "layout-detection")]
        {
            Some(single_stage_pipeline_for_layout(&ocr_config_owned))
        }
        #[cfg(not(feature = "layout-detection"))]
        {
            None
        }
    } else {
        None
    };

    // The top-level `backend` registry lookup is only needed by the single-backend
    // route below; the pipeline route resolves each of its own stage backends
    // internally via `run_ocr_pipeline`. Resolving it eagerly meant a
    // `vlm_fallback = Always` config (or an explicit `pipeline`) that never touches
    // this top-level backend still failed if it happened to be unregistered
    // (review follow-up to #1341).
    let backend = if effective_pipeline.is_none() {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let registry = registry.read();
        Some(registry.get(&ocr_config_owned.backend)?)
    } else {
        None
    };

    let total = page_indices.len();
    let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::with_capacity(total);
    // Tesseract's own mean confidence for each page it read, 0-100. Captured here because
    // the per-page metadata is gone by the time `ocr_results` is judged.
    let mut page_mean_confidence: ahash::AHashMap<u32, f64> = ahash::AHashMap::new();
    let mut structured_ocr_pages: ahash::AHashMap<u32, crate::types::internal::InternalDocument> =
        ahash::AHashMap::with_capacity(total);
    // Bare, unclassified per-page paragraphs for every OCR'd page, real 1-indexed page
    // number -> that page's paragraphs. Collected across both the pipeline and
    // single-backend routes below so the document-global heading/list heuristic
    // (`heuristically_restructured_ocr_pages`) can run exactly once, after this
    // function's per-page loop ends, over every OCR'd page at once -- the same
    // font-clustering pass `extract_with_ocr_for_page` runs for the OCR-only route,
    // previously unreachable here (#665-adjacent gap; see that function's own doc
    // comment on `skip_document_global_heuristic`).
    let mut ocr_page_paragraphs: ahash::AHashMap<u32, Vec<crate::pdf::structure::types::PdfParagraph>> =
        ahash::AHashMap::with_capacity(total);
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut accumulated_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
    #[cfg(feature = "layout-detection")]
    {
        if let Some(warning) = layout_pass_warning {
            crate::core::diagnostics::push_warning_deduped(&mut accumulated_warnings, warning);
        }
        for warning in layout_pass_glyph_drop_warnings {
            crate::core::diagnostics::push_warning_deduped(&mut accumulated_warnings, warning);
        }
    }
    let mut captured_rasters: Vec<crate::types::ExtractedImage> = Vec::new();

    for batch_start in (0..total).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(total);
        let page_images =
            render_selected_pages_from_document(&render_doc, &page_rotations, &page_indices[batch_start..batch_end])?;

        // Multi-stage pipeline route (#1341): drive each page through `run_ocr_pipeline`
        // so `vlm_fallback` / explicit-pipeline stages apply here, mirroring the image
        // extractor's per-image pipeline path. Bounded to this batch's page count (at
        // most `batch_size`, the resolved worker budget) via a `JoinSet`, mirroring the
        // concurrency shape of the single-backend path below.
        if let Some(ref pipeline) = effective_pipeline {
            let page_images = share_rendered_page_images(page_images);
            // on wasm32 (no OS threads, and extractor/backend futures are `!Send` there —
            // see the matching gate on the single-backend path below). Falls back to the
            // sequential loop there even though `tokio-runtime` may be active.
            #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
            {
                let mut join_set = tokio::task::JoinSet::new();
                for (page_idx, image) in &page_images {
                    let image_arc = Arc::clone(image);
                    let pipeline_clone = pipeline.clone();
                    let config_clone = config.clone();
                    let idx = *page_idx;
                    // This page's own known `/Rotate` value, already resolved by
                    // `open_pdf_for_page_ocr` above -- the sibling single-backend route
                    // below reads the same `page_rotations` slice the same way. Passed
                    // explicitly rather than via `content` because the pipeline call below
                    // hands `run_ocr_pipeline` a single detached image (not the full page
                    // list `extract_with_ocr`'s own content-based auto-detection expects
                    // index-aligned to page 0), so an index-based content lookup there would
                    // resolve the wrong page's rotation (or none at all) for anything but the
                    // document's first OCR'd page (#651).
                    let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                    // See `extract_with_ocr_for_page`'s doc comment on
                    // `points_per_pixel_override`: this call hands the stage a single
                    // detached image with `content: None`, so its own pixel -> point lookup
                    // cannot resolve this page's real MediaBox height and would silently fall
                    // back to `1.0`. Computed from the same `render_doc` / raster height the
                    // sibling single-backend route below rescales bboxes with.
                    let points_per_pixel_override = {
                        let (_, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                        let image_height_px = image_arc.height();
                        (image_height_px > 0).then(|| page_height_pt / image_height_px as f32)
                    };
                    // This page's own pixel-space detection from the whole-document layout
                    // pass above (#665), cloned out here because the spawned task below must
                    // own everything it captures. `extract_with_ocr_for_page` (reached through
                    // `run_ocr_pipeline_for_page`) rescales it to match this exact raster via
                    // `scale_detection_to_dimensions`/`scale_detection_to_ocr_coordinates`, so
                    // a DPI mismatch between the layout pass's own render and this page's OCR
                    // render is handled there, not here.
                    #[cfg(feature = "layout-detection")]
                    let page_detection: Option<crate::layout::DetectionResult> =
                        detection_for_mixed_route_page(layout_detections_for_mixed.as_deref(), *page_idx).cloned();
                    join_set.spawn(async move {
                        #[cfg(feature = "layout-detection")]
                        let page_detection_slice = page_detection.as_ref().map(std::slice::from_ref);
                        let result = Box::pin(run_ocr_pipeline_for_page(
                            None,
                            Some(std::slice::from_ref(image_arc.as_ref())),
                            #[cfg(feature = "layout-detection")]
                            page_detection_slice,
                            &config_clone,
                            &pipeline_clone,
                            None,
                            page_rotation_degrees,
                            true,
                            points_per_pixel_override,
                        ))
                        .await;
                        (idx, result)
                    });
                }
                while let Some(join_result) = join_set.join_next().await {
                    let (page_idx, result) = join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR pipeline task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                    let (text, tables, elements, doc, usage, page_texts, _rasters, formulas, mut page_raw_paragraphs) =
                        result?;
                    accumulated_llm_usage.extend(usage);
                    let page_number = (page_idx + 1) as u32;
                    // `run_ocr_pipeline_for_page` drove exactly one detached image
                    // (`std::slice::from_ref` above), so its own per-page paragraph vec has
                    // at most one entry -- this page's own, still bare/unclassified because
                    // `skip_document_global_heuristic = true` was passed above.
                    if let Some(paragraphs) = page_raw_paragraphs.pop().filter(|p| !p.is_empty()) {
                        ocr_page_paragraphs.insert(page_number, paragraphs);
                    }
                    let page_dims = page_images
                        .iter()
                        .find(|(i, _)| *i == page_idx)
                        .map(|(_, img)| (img.width(), img.height()));
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        if let Some((w, h)) = page_dims {
                            formula_bbox_to_page_points(&mut formula, &render_doc, page_idx, None, w, h);
                        }
                        accumulated_formulas.push(formula);
                    }
                    // `run_ocr_pipeline`/`extract_with_ocr` assemble `text` as if this
                    // lone image were page 0 of the document, so a configured page marker
                    // is stamped "page 1" regardless of the real page number. The raw
                    // `page_texts` entry has no marker injected at that layer, so prefer
                    // fall back to `text` only if the backend returned no page_texts.
                    let page_text = page_texts.into_iter().next().unwrap_or(text);
                    // The pipeline's tables and OCR elements used to be dropped here (#60);
                    // they now ride along on the page's structured document. ~keep
                    // This route also skipped the pixel -> point bbox conversion entirely,
                    // so its bboxes stayed in raster pixels after #1423 fixed the
                    // single-backend route; `build_pipeline_ocr_page_document` applies it.
                    let raster_size_px = page_images
                        .iter()
                        .find(|(rendered_page, _)| *rendered_page == page_idx)
                        .map_or((0, 0), |(_, image)| (image.width(), image.height()));
                    if let Some(mut d) = build_pipeline_ocr_page_document(
                        doc,
                        tables,
                        elements,
                        &page_text,
                        page_number,
                        raster_size_px,
                        page_dimensions_pt(&render_doc, page_idx),
                    ) {
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        structured_ocr_pages.insert(page_number, d);
                    }
                    ocr_results.insert(page_number, page_text);
                }
            }
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            {
                for (page_idx, image) in &page_images {
                    // See the matching comments on the sibling `JoinSet` branch above (#651,
                    // and `points_per_pixel_override` re. `extract_with_ocr_for_page`'s doc
                    // comment).
                    let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                    let points_per_pixel_override = {
                        let (_, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                        let image_height_px = image.height();
                        (image_height_px > 0).then(|| page_height_pt / image_height_px as f32)
                    };
                    // See the matching comment on the sibling `JoinSet` branch above (#665).
                    #[cfg(feature = "layout-detection")]
                    let page_detection: Option<&crate::layout::DetectionResult> =
                        detection_for_mixed_route_page(layout_detections_for_mixed.as_deref(), *page_idx);
                    let (text, tables, elements, doc, usage, page_texts, _rasters, formulas, mut page_raw_paragraphs) =
                        Box::pin(run_ocr_pipeline_for_page(
                            None,
                            Some(std::slice::from_ref(image.as_ref())),
                            #[cfg(feature = "layout-detection")]
                            page_detection.map(std::slice::from_ref),
                            config,
                            pipeline,
                            None,
                            page_rotation_degrees,
                            true,
                            points_per_pixel_override,
                        ))
                        .await?;
                    accumulated_llm_usage.extend(usage);
                    let page_number = (*page_idx + 1) as u32;
                    if let Some(paragraphs) = page_raw_paragraphs.pop().filter(|p| !p.is_empty()) {
                        ocr_page_paragraphs.insert(page_number, paragraphs);
                    }
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        formula_bbox_to_page_points(
                            &mut formula,
                            &render_doc,
                            *page_idx,
                            None,
                            image.width(),
                            image.height(),
                        );
                        accumulated_formulas.push(formula);
                    }
                    let page_text = page_texts.into_iter().next().unwrap_or(text);
                    if let Some(mut d) = build_pipeline_ocr_page_document(
                        doc,
                        tables,
                        elements,
                        &page_text,
                        page_number,
                        (image.width(), image.height()),
                        page_dimensions_pt(&render_doc, *page_idx),
                    ) {
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        structured_ocr_pages.insert(page_number, d);
                    }
                    ocr_results.insert(page_number, page_text);
                }
            }
            if capture_rasters {
                for (page_idx, image) in &page_images {
                    let rgb = image.to_rgb8();
                    let (w, h) = rgb.dimensions();
                    let mut buf = Cursor::new(Vec::new());
                    PngEncoder::new(&mut buf)
                        .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode page {} raster: {}", page_idx + 1, e),
                            source: None,
                        })?;
                    captured_rasters.push(build_page_raster_image(
                        *page_idx,
                        bytes::Bytes::from(buf.into_inner()),
                        w,
                        h,
                    ));
                }
            }
            continue;
        }

        // Reached only when `effective_pipeline` is `None`, so `backend` was resolved above.
        let backend = backend
            .as_ref()
            .expect("backend is resolved above whenever effective_pipeline is None");
        let orientation_handling = backend.page_orientation_handling();
        let batch_slice = &page_images;

        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        let encoded: crate::Result<Vec<EncodedPage>> = batch_slice
            .par_iter()
            .map(|(page_idx, image)| {
                let rgb = image.to_rgb8();
                let (w, h) = rgb.dimensions();
                let mut buf = Cursor::new(Vec::new());
                PngEncoder::new(&mut buf)
                    .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                    .map_err(|e| crate::XbergError::Parsing {
                        message: format!("Failed to encode page {} for OCR: {}", page_idx + 1, e),
                        source: None,
                    })?;
                Ok((*page_idx, Arc::new(buf.into_inner()), w, h))
            })
            .collect();
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        let encoded: crate::Result<Vec<EncodedPage>> = batch_slice
            .iter()
            .map(|(page_idx, image)| {
                let rgb = image.to_rgb8();
                let (w, h) = rgb.dimensions();
                let mut buf = Cursor::new(Vec::new());
                PngEncoder::new(&mut buf)
                    .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                    .map_err(|e| crate::XbergError::Parsing {
                        message: format!("Failed to encode page {} for OCR: {}", page_idx + 1, e),
                        source: None,
                    })?;
                Ok((*page_idx, Arc::new(buf.into_inner()), w, h))
            })
            .collect();
        let encoded = encoded?;
        drop(page_images);

        // `tokio::task::JoinSet::spawn` requires `Send` futures, but extractor/backend futures
        // are `!Send` on wasm32 (async_trait(?Send), see plugins/extractor/trait.rs) — and
        // wasm32 has no OS threads to run them on regardless. Fall back to the sequential path
        // there even though `tokio-runtime` is active (it's pulled in by
        // `chunking-tokenizers`/`static-embeddings`, not concurrency support). ~keep
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        {
            let mut join_set = tokio::task::JoinSet::new();
            for (page_idx, data, width, height) in &encoded {
                let backend_clone = Arc::clone(backend);
                let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                let config_clone =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees).into_owned();
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    upright_raster_for_backend(data, *width, *height, page_rotation_degrees, orientation_handling)?;
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(upright_data, &config_clone).await;
                    (idx, correction_degrees, upright_width, upright_height, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, correction_degrees, upright_width, upright_height, result) =
                    join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                let mut extraction_result = result?;
                undo_upright_raster_correction(
                    &mut extraction_result,
                    correction_degrees,
                    upright_width,
                    upright_height,
                );
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                let page_dims = encoded
                    .iter()
                    .find(|(encoded_page, ..)| *encoded_page == page_idx)
                    .map(|(_, _, w, h)| (*w, *h));
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = Some((page_idx + 1) as u32);
                    if let Some((w, h)) = page_dims {
                        formula_bbox_to_page_points(
                            &mut formula,
                            &render_doc,
                            page_idx,
                            Some(&extraction_result.metadata),
                            w,
                            h,
                        );
                    }
                    accumulated_formulas.push(formula);
                }
                // The backend's own warnings used to be dropped on this route (#60).
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                let (width, height) = encoded
                    .iter()
                    .find(|(encoded_page, ..)| *encoded_page == page_idx)
                    .map_or((0, 0), |(_, _, w, h)| (*w, *h));
                let (page_width_pt, page_height_pt) = page_dimensions_pt(&render_doc, page_idx);
                if let Some((mut page_doc, paragraphs)) = build_mixed_ocr_page_document(
                    &mut extraction_result,
                    (page_idx + 1) as u32,
                    width,
                    height,
                    page_width_pt,
                    page_height_pt,
                ) {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    if !paragraphs.is_empty() {
                        ocr_page_paragraphs.insert((page_idx + 1) as u32, paragraphs);
                    }
                    structured_ocr_pages.insert((page_idx + 1) as u32, page_doc);
                }
                if let Some(conf) = mean_text_conf_of(&extraction_result.metadata.additional) {
                    page_mean_confidence.insert((page_idx + 1) as u32, conf);
                }
                ocr_results.insert((page_idx + 1) as u32, extraction_result.content);
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, data, width, height) in &encoded {
                let page_rotation_degrees = page_rotations.get(*page_idx).copied().unwrap_or(0);
                let config_for_page = ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees);
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    upright_raster_for_backend(data, *width, *height, page_rotation_degrees, orientation_handling)?;
                let mut extraction_result = backend
                    .process_image(upright_data.as_slice(), config_for_page.as_ref())
                    .await?;
                undo_upright_raster_correction(
                    &mut extraction_result,
                    correction_degrees,
                    upright_width,
                    upright_height,
                );
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = Some((*page_idx + 1) as u32);
                    formula_bbox_to_page_points(
                        &mut formula,
                        &render_doc,
                        *page_idx,
                        Some(&extraction_result.metadata),
                        *width,
                        *height,
                    );
                    accumulated_formulas.push(formula);
                }
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                let (page_width_pt, page_height_pt) = page_dimensions_pt(&render_doc, *page_idx);
                if let Some((mut page_doc, paragraphs)) = build_mixed_ocr_page_document(
                    &mut extraction_result,
                    (*page_idx + 1) as u32,
                    *width,
                    *height,
                    page_width_pt,
                    page_height_pt,
                ) {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    if !paragraphs.is_empty() {
                        ocr_page_paragraphs.insert((*page_idx + 1) as u32, paragraphs);
                    }
                    structured_ocr_pages.insert((*page_idx + 1) as u32, page_doc);
                }
                if let Some(conf) = mean_text_conf_of(&extraction_result.metadata.additional) {
                    page_mean_confidence.insert((*page_idx + 1) as u32, conf);
                }
                ocr_results.insert((*page_idx + 1) as u32, extraction_result.content);
            }
        }

        if capture_rasters {
            for (page_idx, png_arc, w, h) in &encoded {
                let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                captured_rasters.push(build_page_raster_image(*page_idx, png_bytes, *w, *h));
            }
        }
    }

    // Drop pages whose OCR is recognition noise before they are accepted as replacements.
    // A rejected page keeps its native text, which for a scanned drawing is nothing — the
    // correct outcome, since the page has no readable words to begin with.
    let ocr_output_thresholds = config
        .ocr
        .as_ref()
        .and_then(|ocr| ocr.quality_thresholds.clone())
        .unwrap_or_default();
    let backend_name = config.ocr.as_ref().map(|ocr| ocr.backend.as_str()).unwrap_or_default();
    let confidence_semantics = resolve_confidence_semantics(backend.as_ref(), backend_name);
    ocr_results.retain(|page_number, text| {
        let confidence = page_mean_confidence.get(page_number).copied();
        tracing::debug!(page = *page_number, ?confidence, "OCR page mean confidence");

        // The engine's own confidence is the sharper instrument, so it decides — but only
        // where its scale is known to mean legibility (`ConfidenceSemantics::Legibility`).
        // Elsewhere the text heuristic runs; an `Uncalibrated` or `None` backend must never
        // have its (possibly meaningless, possibly inverted) number empty a document.
        let rejected_by_confidence = confidence_gate_rejects(
            confidence_semantics,
            confidence,
            ocr_output_thresholds.min_ocr_mean_confidence,
        );
        let judged_by_confidence = confidence_gate_applies(confidence_semantics, confidence);
        if !rejected_by_confidence && (judged_by_confidence || !is_ocr_recognition_noise(text, &ocr_output_thresholds))
        {
            return true;
        }
        if rejected_by_confidence {
            let conf = confidence.unwrap_or_default();
            let scale_max = match confidence_semantics {
                crate::plugins::ConfidenceSemantics::Legibility { scale_max } => scale_max,
                _ => 100.0,
            };
            tracing::warn!(
                page = *page_number,
                mean_confidence = conf,
                scale_max,
                threshold = ocr_output_thresholds.min_ocr_mean_confidence,
                "rejecting OCR output as recognition noise; the page contributes no text"
            );
            accumulated_warnings.push(crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("ocr"),
                message: std::borrow::Cow::Owned(format!(
                    "Page {page_number} produced OCR output the engine had little confidence in \
                     (mean confidence {:.0}% of scale, threshold {:.0}%); the page is most likely \
                     a drawing or diagram. Its text was discarded.",
                    (conf / scale_max) * 100.0,
                    ocr_output_thresholds.min_ocr_mean_confidence
                )),
            });
            return false;
        }
        let stats = ocr_output_stats(text, &ocr_output_thresholds);
        tracing::warn!(
            page = *page_number,
            words = stats.word_count,
            fragmented_word_ratio = stats.fragmented_word_ratio,
            threshold = ocr_output_thresholds.max_ocr_output_fragmented_word_ratio,
            "rejecting OCR output as recognition noise; the page contributes no text"
        );
        accumulated_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("ocr"),
            message: std::borrow::Cow::Owned(format!(
                "Page {} produced OCR output that is recognition noise rather than text \
                 ({:.0}% of {} words are 1-2 characters, threshold {:.0}%); the page is most \
                 likely a drawing or diagram. Its text was discarded.",
                page_number,
                stats.fragmented_word_ratio * 100.0,
                stats.word_count,
                ocr_output_thresholds.max_ocr_output_fragmented_word_ratio * 100.0
            )),
        });
        false
    });

    for text in ocr_results.values_mut() {
        if let std::borrow::Cow::Owned(repaired) = repair_ocr_list_markers(text) {
            *text = repaired;
        }
    }

    let accepted_replacements = accepted_ocr_page_replacements(native_text, boundaries, &ocr_results);
    structured_ocr_pages.retain(|page, _| accepted_replacements.contains_key(page));

    // Document-global heading/list heuristic (same font-clustering pass the OCR-only
    // route runs via `extract_with_ocr_for_page`, previously unreachable from this mixed
    // route -- see `ocr_page_paragraphs`'s own comment above). Runs once, over every
    // accepted OCR'd page's paragraphs at once: the heuristic clusters font sizes
    // document-wide and needs several pages in hand, which is exactly what the per-page
    // callers above could not give it on their own.
    if !structured_ocr_pages.is_empty() {
        let mut pages_for_heuristic: Vec<Vec<crate::pdf::structure::types::PdfParagraph>> =
            vec![Vec::new(); page_count];
        for (&page_number, paragraphs) in &ocr_page_paragraphs {
            // `structured_ocr_pages` (not `ocr_page_paragraphs`) is the source of truth for
            // which pages are still in play: the confidence/noise retain above may have
            // dropped a page after its paragraphs were already collected.
            if structured_ocr_pages.contains_key(&page_number)
                && let Some(slot) = pages_for_heuristic.get_mut((page_number - 1) as usize)
            {
                *slot = paragraphs.clone();
            }
        }
        let tables_for_heuristic: Vec<crate::types::Table> = structured_ocr_pages
            .values()
            .flat_map(|doc| doc.tables.iter().cloned())
            .collect();
        let mut ocr_page_numbers: Vec<u32> = structured_ocr_pages.keys().copied().collect();
        ocr_page_numbers.sort_unstable();
        let mut split_pages = heuristically_restructured_ocr_pages(&pages_for_heuristic, &tables_for_heuristic, config)
            .map(|combined_doc| split_document_global_ocr_structure_by_page(combined_doc, &ocr_page_numbers))
            .unwrap_or_default();
        for page_number in &ocr_page_numbers {
            let Some(existing) = structured_ocr_pages.get(page_number) else {
                continue;
            };
            let new_page_doc = match split_pages.remove(page_number) {
                // The heuristic's combined document has no notion of the backend's raw
                // per-word OCR elements or this page's earlier warnings -- both come
                // from the fallback per-page document already built above; only the
                // *structural* elements (headings/paragraphs/list items/tables) come
                // from the document-global pass.
                Some(mut new_page_doc) => {
                    new_page_doc.prebuilt_ocr_elements = existing.prebuilt_ocr_elements.clone();
                    new_page_doc.processing_warnings = existing.processing_warnings.clone();
                    new_page_doc
                }
                // The heuristic either didn't run at all for this document (Plain output, or
                // some other page was already ML-layout-classified -- `already_structured` is
                // document-wide) or dropped this specific page. Either way this page's
                // paragraphs never got ANY list-item classification (#713): apply a
                // text-marker-only fallback pass, which never touches `heading_level`, so it
                // cannot regress heading detection the way pre-classifying before the
                // heuristic call would.
                None => {
                    let mut paragraphs = pages_for_heuristic
                        .get((*page_number - 1) as usize)
                        .cloned()
                        .unwrap_or_default();
                    apply_ocr_text_list_fallback(&mut paragraphs);
                    // #729: recover a bare marker no ML hint ever classified, e.g. a
                    // marker paragraph whose body landed several paragraphs away --
                    // see `adapters::reattach_detached_ocr_list_markers`'s doc comment.
                    // #760: threaded with this page's known PDF `/Rotate` so the
                    // reattachment's baseline/indent comparison runs in the rotation-
                    // corrected frame instead of the raw raster one.
                    let page_rotation_degrees = page_rotations.get((*page_number - 1) as usize).copied().unwrap_or(0);
                    crate::pdf::structure::adapters::reattach_detached_ocr_list_markers(
                        &mut paragraphs,
                        page_rotation_degrees,
                    );
                    let mut new_page_doc = crate::pdf::structure::assemble_internal_document(
                        vec![paragraphs],
                        &existing.tables,
                        Some(&existing.images),
                        &[],
                    );
                    new_page_doc.prebuilt_ocr_elements = existing.prebuilt_ocr_elements.clone();
                    new_page_doc.processing_warnings = existing.processing_warnings.clone();
                    new_page_doc
                }
            };
            structured_ocr_pages.insert(*page_number, new_page_doc);
        }
    }

    let result = apply_ocr_page_replacements(native_text, boundaries, &accepted_replacements);

    Ok((
        result,
        accepted_replacements,
        structured_ocr_pages,
        accumulated_llm_usage,
        if capture_rasters { Some(captured_rasters) } else { None },
        accumulated_formulas,
        accumulated_warnings,
    ))
}

/// Merge per-page OCR text into the native text, replacing each OCR'd page's
/// byte range in place.
///
/// Boundaries are processed in reverse byte order so earlier offsets stay valid
/// after each replacement. An OCR entry that is empty (or whitespace-only) is
/// skipped rather than applied: an empty OCR result must never overwrite a page's
/// native text, or a page whose backend produced nothing would silently lose its
/// already-extracted content.
// Gated to `ocr` rather than `any(ocr, ocr-pipeline)` to match its only
// callers in the `#[cfg(all(test, feature = "ocr"))]` test module. ~keep
#[cfg(all(test, feature = "ocr"))]
pub(crate) fn merge_ocr_pages_into_native(
    native_text: &str,
    boundaries: &[crate::types::PageBoundary],
    ocr_results: &ahash::AHashMap<u32, String>,
) -> String {
    let accepted = accepted_ocr_page_replacements(native_text, boundaries, ocr_results);
    apply_ocr_page_replacements(native_text, boundaries, &accepted)
}

/// Keep only OCR results that can be applied consistently to every mixed-output
/// representation: non-empty text with a matching, valid UTF-8 page boundary.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn accepted_ocr_page_replacements(
    native_text: &str,
    boundaries: &[crate::types::PageBoundary],
    ocr_results: &ahash::AHashMap<u32, String>,
) -> ahash::AHashMap<u32, String> {
    let mut page_counts = std::collections::HashMap::new();
    for boundary in boundaries {
        *page_counts.entry(boundary.page_number).or_insert(0usize) += 1;
    }

    let mut valid_boundaries: Vec<&crate::types::PageBoundary> = boundaries
        .iter()
        .filter(|boundary| {
            page_counts.get(&boundary.page_number) == Some(&1)
                && boundary.page_number > 0
                && boundary.byte_start <= boundary.byte_end
                && boundary.byte_end <= native_text.len()
                && native_text.is_char_boundary(boundary.byte_start)
                && native_text.is_char_boundary(boundary.byte_end)
        })
        .collect();
    valid_boundaries.sort_unstable_by_key(|boundary| (boundary.byte_start, boundary.byte_end));

    let mut overlapping_pages = std::collections::HashSet::new();
    let mut active: Option<&crate::types::PageBoundary> = None;
    for boundary in &valid_boundaries {
        if let Some(previous) = active
            && boundary.byte_start < previous.byte_end
        {
            overlapping_pages.insert(previous.page_number);
            overlapping_pages.insert(boundary.page_number);
        }
        if active.is_none_or(|previous| boundary.byte_end > previous.byte_end) {
            active = Some(boundary);
        }
    }

    let valid_pages: std::collections::HashSet<u32> = valid_boundaries
        .into_iter()
        .filter(|boundary| !overlapping_pages.contains(&boundary.page_number))
        .map(|boundary| boundary.page_number)
        .collect();

    for (&page, text) in ocr_results {
        if !text.trim().is_empty() && !valid_pages.contains(&page) {
            tracing::warn!(
                page,
                "rejecting mixed OCR page without one valid, non-overlapping text boundary"
            );
        }
    }

    // An accepted replacement OVERWRITES the page's native byte range, so a page whose
    // OCR came back effectively empty must not be accepted: doing so deletes whatever the
    // native text layer had and makes the OCR run return *less* than not running it at all.
    // `!text.trim().is_empty()` is too weak a bar -- a single stray character cleared it,
    // which is exactly what a blank/failed page render produces. Use the same blank
    // threshold the rest of the crate uses so one definition governs both. ~keep
    ocr_results
        .iter()
        .filter(|(page, text)| {
            if !valid_pages.contains(page) {
                return false;
            }
            if crate::extraction::blank_detection::is_page_text_blank(text) {
                tracing::warn!(
                    page = **page,
                    chars = text.trim().chars().count(),
                    "rejecting mixed OCR page whose OCR output is blank; keeping native text for this page"
                );
                return false;
            }
            true
        })
        .map(|(&page, text)| (page, text.clone()))
        .collect()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn apply_ocr_page_replacements(
    native_text: &str,
    boundaries: &[crate::types::PageBoundary],
    accepted: &ahash::AHashMap<u32, String>,
) -> String {
    let mut result = native_text.to_string();

    let mut sorted_boundaries: Vec<&crate::types::PageBoundary> = boundaries
        .iter()
        .filter(|boundary| accepted.contains_key(&boundary.page_number))
        .collect();
    sorted_boundaries.sort_unstable_by_key(|boundary| std::cmp::Reverse((boundary.byte_start, boundary.page_number)));

    for boundary in sorted_boundaries {
        if let Some(ocr_text) = accepted.get(&boundary.page_number) {
            result.replace_range(boundary.byte_start..boundary.byte_end, ocr_text);
        }
    }

    result
}

/// Re-map page boundaries onto the text produced by `apply_ocr_page_replacements`.
///
/// Replacing a page's byte range with OCR text of a different length shifts every
/// later offset, so the input boundaries describe the NATIVE text and are wrong for
/// the merged result. Without this, anything downstream that maps a byte offset back
/// to a page number -- including page tagging on the flat-document path -- either
/// mis-attributes content or has to give up and emit no pages at all. Walks forward
/// accumulating the per-page delta; unreplaced pages simply shift by the running
/// total, so gaps between boundaries are preserved. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn boundaries_after_replacements(
    boundaries: &[crate::types::PageBoundary],
    accepted: &ahash::AHashMap<u32, String>,
) -> Vec<crate::types::PageBoundary> {
    let mut adjusted: Vec<crate::types::PageBoundary> = boundaries.to_vec();
    adjusted.sort_by_key(|boundary| boundary.byte_start);

    let mut delta: isize = 0;
    for boundary in &mut adjusted {
        let original_start = boundary.byte_start;
        let original_end = boundary.byte_end;
        boundary.byte_start = original_start.saturating_add_signed(delta);
        if let Some(ocr_text) = accepted.get(&boundary.page_number) {
            let old_len = original_end.saturating_sub(original_start);
            delta += ocr_text.len() as isize - old_len as isize;
        }
        boundary.byte_end = original_end.saturating_add_signed(delta);
    }

    adjusted
}

/// Replace native text-flow elements on OCR'd pages while preserving the
/// structured document's tables, images, and reading-order position.
///
/// PDF list markers do not carry page numbers, so page ownership is inferred
/// from balanced container spans before filtering. Page breaks are rebuilt
/// from the resulting page sequence, and relationships are remapped to the
/// final element indices (or dropped when either indexed endpoint was removed).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn merge_ocr_pages_into_internal_document(
    doc: &mut crate::types::internal::InternalDocument,
    ocr_results: &ahash::AHashMap<u32, String>,
) {
    merge_structured_ocr_pages_into_internal_document(doc, ocr_results, &ahash::AHashMap::new());
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn merge_structured_ocr_pages_into_internal_document(
    doc: &mut crate::types::internal::InternalDocument,
    ocr_results: &ahash::AHashMap<u32, String>,
    structured_pages: &ahash::AHashMap<u32, crate::types::internal::InternalDocument>,
) {
    let replacements: std::collections::BTreeMap<u32, &str> = ocr_results
        .iter()
        .filter_map(|(&page, text)| (!text.trim().is_empty()).then_some((page, text.as_str())))
        .collect();
    if replacements.is_empty() {
        return;
    }

    let containers = analyze_container_markers(&doc.elements);
    let anchors = replacement_anchors(&doc.elements, &containers.inferred_pages, &replacements);
    // Assets carried by a per-page OCR document are re-indexed into the parent's
    // collections instead of being discarded. Discarding them used to force the
    // raw-text fallback in `append_ocr_replacements`, which dropped every table the
    // OCR'd page produced (#57) and destroyed the asset-to-page association (#59).
    let mut assets = MergedOcrAssets::new(doc.tables.len() as u32, doc.images.len() as u32);
    let planned = plan_merged_elements(
        &doc.elements,
        &containers,
        &replacements,
        structured_pages,
        &anchors,
        &mut assets,
    );
    let (rebuilt, old_to_new) = rebuild_planned_elements(planned, doc.elements.len());
    remap_relationships(&mut doc.relationships, &old_to_new, &rebuilt);
    doc.elements = rebuilt;
    doc.tables.extend(assets.tables);
    doc.images.extend(assets.images);
    if !assets.ocr_elements.is_empty() {
        doc.prebuilt_ocr_elements
            .get_or_insert_with(Vec::new)
            .extend(assets.ocr_elements);
    }
}

/// Tables, images and OCR elements lifted out of per-page OCR documents and
/// re-indexed into the parent document's collections.
///
/// `table_base` / `image_base` are the parent's collection lengths before the
/// merge, so a page-local index `i` becomes `base + already_merged + i`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
struct MergedOcrAssets {
    table_base: u32,
    image_base: u32,
    tables: Vec<crate::types::Table>,
    images: Vec<crate::types::ExtractedImage>,
    ocr_elements: Vec<crate::types::OcrElement>,
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
impl MergedOcrAssets {
    fn new(table_base: u32, image_base: u32) -> Self {
        Self {
            table_base,
            image_base,
            tables: Vec::new(),
            images: Vec::new(),
            ocr_elements: Vec::new(),
        }
    }

    fn next_table_index(&self) -> u32 {
        self.table_base + self.tables.len() as u32
    }

    fn next_image_index(&self) -> u32 {
        self.image_base + self.images.len() as u32
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
struct PlannedOcrElement {
    element: crate::types::internal::InternalElement,
    old_index: Option<usize>,
    page: Option<u32>,
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn replacement_anchors<'a>(
    elements: &[crate::types::internal::InternalElement],
    inferred_pages: &[Option<u32>],
    replacements: &std::collections::BTreeMap<u32, &'a str>,
) -> std::collections::BTreeMap<usize, Vec<(u32, &'a str)>> {
    let mut anchors = std::collections::BTreeMap::new();
    for (&page, &text) in replacements {
        let anchor = elements
            .iter()
            .enumerate()
            .find(|(index, element)| {
                inferred_pages[*index]
                    .or(element.page)
                    .is_some_and(|element_page| element_page >= page)
            })
            .map_or(elements.len(), |(index, _)| index);
        anchors.entry(anchor).or_insert_with(Vec::new).push((page, text));
    }
    anchors
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn plan_merged_elements(
    elements: &[crate::types::internal::InternalElement],
    containers: &ContainerMarkerAnalysis,
    replacements: &std::collections::BTreeMap<u32, &str>,
    structured_pages: &ahash::AHashMap<u32, crate::types::internal::InternalDocument>,
    anchors: &std::collections::BTreeMap<usize, Vec<(u32, &str)>>,
    assets: &mut MergedOcrAssets,
) -> Vec<PlannedOcrElement> {
    use crate::types::internal::ElementKind;

    let mut planned = Vec::with_capacity(elements.len() + replacements.len());
    for (old_index, element) in elements.iter().enumerate() {
        append_ocr_replacements(&mut planned, anchors.get(&old_index), structured_pages, assets);
        if containers.drop_marker[old_index] {
            continue;
        }
        if matches!(element.kind, ElementKind::PageBreak) {
            continue;
        }
        let page = element.page.or(containers.inferred_pages[old_index]);
        let preserve_asset = matches!(element.kind, ElementKind::Image { .. });
        if !preserve_asset && page.is_some_and(|page| replacements.contains_key(&page)) {
            continue;
        }
        let mut element = element.clone();
        if matches!(element.kind, ElementKind::Image { .. })
            && page.is_some_and(|page| replacements.contains_key(&page))
        {
            element.suppress_image_ocr_rendering();
        }
        planned.push(PlannedOcrElement {
            element,
            old_index: Some(old_index),
            page,
        });
    }
    append_ocr_replacements(&mut planned, anchors.get(&elements.len()), structured_pages, assets);
    planned
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn append_ocr_replacements(
    planned: &mut Vec<PlannedOcrElement>,
    replacements: Option<&Vec<(u32, &str)>>,
    structured_pages: &ahash::AHashMap<u32, crate::types::internal::InternalDocument>,
    assets: &mut MergedOcrAssets,
) {
    use crate::types::internal::{ElementKind, InternalElement};
    use crate::types::ocr_elements::OcrElementLevel;

    for &(page, text) in replacements.into_iter().flatten() {
        // Usability is decided before re-indexing so a rejected page never leaks its
        // tables/images into `assets`.
        let structured_page = structured_pages.get(&page).filter(|doc| {
            !doc.tables.is_empty()
                || !doc.images.is_empty()
                || doc
                    .elements
                    .iter()
                    .any(|element| !matches!(element.kind, ElementKind::PageBreak) && !element.text.trim().is_empty())
        });
        if let Some(structured_page) = structured_page {
            let elements = reindex_structured_ocr_page(structured_page, page, assets);
            planned.extend(elements.into_iter().map(|element| PlannedOcrElement {
                element,
                old_index: None,
                page: Some(page),
            }));
            continue;
        }
        // Backend text verbatim (see `flat_ocr_page_document`): normalize before splitting.
        let text = crate::extraction::transform::normalize_line_endings(text);
        for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
            let element = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                paragraph,
                0,
            )
            .with_page(page);
            planned.push(PlannedOcrElement {
                element,
                old_index: None,
                page: Some(page),
            });
        }
    }
}

/// Move an OCR'd page's tables, images and OCR elements into the parent document's
/// collections and rewrite the page's element references to the new parent indices.
///
/// Page-local `Table { table_index }` / `Image { image_index }` references are only
/// meaningful against the page document's own collections, so they must be rebased
/// before the elements are spliced into the parent (#59). Assets the page document
/// carries but never references from its element list still get a reference emitted,
/// so a table produced by OCR cannot silently vanish (#57).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn reindex_structured_ocr_page(
    page_doc: &crate::types::internal::InternalDocument,
    page: u32,
    assets: &mut MergedOcrAssets,
) -> Vec<crate::types::internal::InternalElement> {
    use crate::types::internal::{ElementKind, InternalElement};

    let table_base = assets.next_table_index();
    let image_base = assets.next_image_index();

    for table in &page_doc.tables {
        let mut table = table.clone();
        table.page_number = page;
        assets.tables.push(table);
    }
    for (local_index, image) in page_doc.images.iter().enumerate() {
        let mut image = image.clone();
        image.page_number = Some(page);
        image.image_index = image_base + local_index as u32;
        assets.images.push(image);
    }
    if let Some(page_ocr_elements) = page_doc.prebuilt_ocr_elements.as_ref() {
        assets
            .ocr_elements
            .extend(page_ocr_elements.iter().cloned().map(|mut element| {
                element.page_number = page;
                element
            }));
    }

    let mut referenced_tables = vec![false; page_doc.tables.len()];
    let mut referenced_images = vec![false; page_doc.images.len()];
    let mut elements = Vec::with_capacity(page_doc.elements.len());
    for element in &page_doc.elements {
        if matches!(element.kind, ElementKind::PageBreak) {
            continue;
        }
        let mut element = element.clone();
        match element.kind {
            ElementKind::Table { table_index } => {
                let Some(referenced) = referenced_tables.get_mut(table_index as usize) else {
                    // Dangling page-local reference: the table it points at does not exist.
                    continue;
                };
                *referenced = true;
                element.kind = ElementKind::Table {
                    table_index: table_base + table_index,
                };
            }
            ElementKind::Image { image_index } => {
                let Some(referenced) = referenced_images.get_mut(image_index as usize) else {
                    continue;
                };
                *referenced = true;
                element.kind = ElementKind::Image {
                    image_index: image_base + image_index,
                };
            }
            _ => {}
        }
        element.page = Some(page);
        elements.push(element);
    }

    for (local_index, referenced) in referenced_tables.iter().enumerate() {
        if !*referenced {
            elements.push(
                InternalElement::text(
                    ElementKind::Table {
                        table_index: table_base + local_index as u32,
                    },
                    "",
                    0,
                )
                .with_page(page),
            );
        }
    }
    for (local_index, referenced) in referenced_images.iter().enumerate() {
        if !*referenced {
            elements.push(
                InternalElement::text(
                    ElementKind::Image {
                        image_index: image_base + local_index as u32,
                    },
                    "",
                    0,
                )
                .with_page(page),
            );
        }
    }

    elements
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn rebuild_planned_elements(
    planned: Vec<PlannedOcrElement>,
    old_len: usize,
) -> (Vec<crate::types::internal::InternalElement>, Vec<Option<u32>>) {
    use crate::types::internal::{ElementKind, InternalElement};

    let mut old_to_new = vec![None; old_len];
    let mut rebuilt = Vec::with_capacity(planned.len());
    let mut previous_page = None;
    for planned_element in planned {
        if let (Some(previous), Some(current)) = (previous_page, planned_element.page)
            && previous != current
        {
            rebuilt.push(InternalElement::text(ElementKind::PageBreak, "", 0));
        }
        if let Some(page) = planned_element.page {
            previous_page = Some(page);
        }
        if let Some(old_index) = planned_element.old_index {
            old_to_new[old_index] = Some(rebuilt.len() as u32);
        }
        rebuilt.push(planned_element.element);
    }
    for (index, element) in rebuilt.iter_mut().enumerate() {
        *element = element.clone().with_index(index as u32);
    }
    (rebuilt, old_to_new)
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn remap_relationships(
    relationships: &mut Vec<crate::types::internal::Relationship>,
    old_to_new: &[Option<u32>],
    rebuilt: &[crate::types::internal::InternalElement],
) {
    use crate::types::internal::RelationshipTarget;

    let retained_anchors: std::collections::HashSet<&str> =
        rebuilt.iter().filter_map(|element| element.anchor.as_deref()).collect();
    relationships.retain_mut(|relationship| {
        let Some(source) = old_to_new.get(relationship.source as usize).copied().flatten() else {
            return false;
        };
        relationship.source = source;
        match &mut relationship.target {
            RelationshipTarget::Index(target) => {
                let Some(remapped) = old_to_new.get(*target as usize).copied().flatten() else {
                    return false;
                };
                *target = remapped;
            }
            RelationshipTarget::Key(key) if !retained_anchors.contains(key.as_str()) => return false,
            RelationshipTarget::Key(_) => {}
        }
        true
    });
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
struct ContainerMarkerAnalysis {
    inferred_pages: Vec<Option<u32>>,
    drop_marker: Vec<bool>,
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn analyze_container_markers(elements: &[crate::types::internal::InternalElement]) -> ContainerMarkerAnalysis {
    use crate::types::internal::ElementKind;

    fn matching_container(start: ElementKind, end: ElementKind) -> bool {
        matches!(
            (start, end),
            (ElementKind::ListStart { .. }, ElementKind::ListEnd)
                | (ElementKind::QuoteStart, ElementKind::QuoteEnd)
                | (ElementKind::GroupStart, ElementKind::GroupEnd)
        )
    }

    let mut analysis = ContainerMarkerAnalysis {
        inferred_pages: vec![None; elements.len()],
        drop_marker: vec![false; elements.len()],
    };
    let mut stack: Vec<(usize, ElementKind)> = Vec::new();
    for (index, element) in elements.iter().enumerate() {
        if element.kind.is_container_start() {
            stack.push((index, element.kind));
            continue;
        }
        if !element.kind.is_container_end() {
            continue;
        }
        let Some(&(start_index, start_kind)) = stack.last() else {
            analysis.drop_marker[index] = true;
            continue;
        };
        if !matching_container(start_kind, element.kind) {
            analysis.drop_marker[index] = true;
            continue;
        }
        stack.pop();
        let pages: std::collections::HashSet<u32> = elements[start_index..=index]
            .iter()
            .filter_map(|element| element.page)
            .collect();
        if pages.len() == 1 {
            let page = pages.iter().next().copied();
            analysis.inferred_pages[start_index] = page;
            analysis.inferred_pages[index] = page;
        } else {
            analysis.drop_marker[start_index] = true;
            analysis.drop_marker[index] = true;
        }
    }
    for (start_index, _) in stack {
        analysis.drop_marker[start_index] = true;
    }
    analysis
}

// The OCR metadata keys come from `crate::ocr_metadata_keys`, which is ungated, rather
// than from `crate::ocr`: this PDF OCR path also compiles under `ocr-pipeline` (VLM OCR,
// e.g. the `binstall` CLI) or under `layout-detection` alone (layout without any OCR
// backend enabled), where the `ocr` module — gated on `ocr`/`ocr-wasm` — is absent. ~keep
#[cfg(any(
    feature = "ocr",
    feature = "ocr-wasm",
    all(feature = "ocr-pipeline", feature = "pdf")
))]
use crate::ocr_metadata_keys::{OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY, OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY};
// Same rationale, scoped to `layout-detection` only: `resolved_ocr_correction_degrees` and
// `transform_ocr_elements_to_render_space` (both `layout-detection`-only) are the sole
// readers of these two key names in this file.
#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
use crate::ocr_metadata_keys::{OCR_AUTO_ROTATED_METADATA_KEY, OCR_ORIENTATION_DEGREES_METADATA_KEY};

#[cfg(any(
    feature = "ocr",
    feature = "ocr-wasm",
    all(feature = "ocr-pipeline", feature = "pdf")
))]
fn valid_ocr_layout_dimension(value: &serde_json::Value) -> Option<u32> {
    let value = value.as_f64()?;
    if !value.is_finite() || value <= 0.0 || value > u32::MAX as f64 || value.fract() != 0.0 {
        return None;
    }
    Some(value as u32)
}

#[cfg(any(
    feature = "ocr",
    feature = "ocr-wasm",
    all(feature = "ocr-pipeline", feature = "pdf")
))]
fn processed_ocr_layout_dimensions(metadata: &crate::types::Metadata) -> Option<(u32, u32)> {
    let width = metadata
        .additional
        .get(OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY)
        .and_then(valid_ocr_layout_dimension);
    let height = metadata
        .additional
        .get(OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY)
        .and_then(valid_ocr_layout_dimension);

    match (width, height) {
        (Some(width), Some(height)) => Some((width, height)),
        _ => None,
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-wasm"))]
fn resolved_ocr_layout_dimensions(
    metadata: &crate::types::Metadata,
    render_width: u32,
    render_height: u32,
) -> (u32, u32) {
    processed_ocr_layout_dimensions(metadata).unwrap_or((render_width, render_height))
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn scale_detection_to_dimensions(
    detection: &crate::layout::DetectionResult,
    target_width: u32,
    target_height: u32,
) -> crate::layout::DetectionResult {
    if detection.page_width == 0 || detection.page_height == 0 || target_width == 0 || target_height == 0 {
        return detection.clone();
    }

    let scale_x = target_width as f32 / detection.page_width as f32;
    let scale_y = target_height as f32 / detection.page_height as f32;
    let mut scaled = detection.clone();
    scaled.page_width = target_width;
    scaled.page_height = target_height;
    for region in &mut scaled.detections {
        region.bbox.x1 *= scale_x;
        region.bbox.y1 *= scale_y;
        region.bbox.x2 *= scale_x;
        region.bbox.y2 *= scale_y;
    }
    scaled
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn resolved_ocr_correction_degrees(metadata: &crate::types::Metadata) -> Option<u16> {
    if !metadata
        .additional
        .get(OCR_AUTO_ROTATED_METADATA_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let orientation = metadata
        .additional
        .get(OCR_ORIENTATION_DEGREES_METADATA_KEY)
        .and_then(serde_json::Value::as_i64)?;
    if !matches!(orientation, 0 | 90 | 180 | 270) {
        return None;
    }
    Some(((360 - orientation) % 360) as u16)
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn rotate_detection(
    mut detection: crate::layout::DetectionResult,
    correction_degrees: u16,
) -> crate::layout::DetectionResult {
    let source_width = detection.page_width as f32;
    let source_height = detection.page_height as f32;
    for region in &mut detection.detections {
        let (x1, y1, x2, y2) = (region.bbox.x1, region.bbox.y1, region.bbox.x2, region.bbox.y2);
        match correction_degrees {
            90 => {
                region.bbox.x1 = source_height - y2;
                region.bbox.y1 = x1;
                region.bbox.x2 = source_height - y1;
                region.bbox.y2 = x2;
            }
            180 => {
                region.bbox.x1 = source_width - x2;
                region.bbox.y1 = source_height - y2;
                region.bbox.x2 = source_width - x1;
                region.bbox.y2 = source_height - y1;
            }
            270 => {
                region.bbox.x1 = y1;
                region.bbox.y1 = source_width - x2;
                region.bbox.x2 = y2;
                region.bbox.y2 = source_width - x1;
            }
            _ => {}
        }
    }
    if matches!(correction_degrees, 90 | 270) {
        std::mem::swap(&mut detection.page_width, &mut detection.page_height);
    }
    detection
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn scale_detection_to_ocr_coordinates(
    detection: &crate::layout::DetectionResult,
    metadata: &crate::types::Metadata,
    render_width: u32,
    render_height: u32,
) -> crate::layout::DetectionResult {
    let Some((final_width, final_height)) = processed_ocr_layout_dimensions(metadata) else {
        return scale_detection_to_dimensions(detection, render_width, render_height);
    };
    let Some(correction_degrees) = resolved_ocr_correction_degrees(metadata) else {
        return scale_detection_to_dimensions(detection, final_width, final_height);
    };
    let (pre_rotation_width, pre_rotation_height) = if matches!(correction_degrees, 90 | 270) {
        (final_height, final_width)
    } else {
        (final_width, final_height)
    };
    let scaled = scale_detection_to_dimensions(detection, pre_rotation_width, pre_rotation_height);
    rotate_detection(scaled, correction_degrees)
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn inverse_rotate_ocr_point(
    x: f64,
    y: f64,
    correction_degrees: u16,
    pre_rotation_width: f64,
    pre_rotation_height: f64,
) -> (f64, f64) {
    match correction_degrees {
        90 => (y, pre_rotation_height - x),
        180 => (pre_rotation_width - x, pre_rotation_height - y),
        270 => (pre_rotation_width - y, x),
        _ => (x, y),
    }
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn transform_ocr_point_to_render(
    point: (u32, u32),
    correction_degrees: u16,
    pre_rotation_dimensions: (u32, u32),
    render_dimensions: (u32, u32),
) -> (u32, u32) {
    let (pre_width, pre_height) = pre_rotation_dimensions;
    let (render_width, render_height) = render_dimensions;
    let (x, y) = inverse_rotate_ocr_point(
        point.0 as f64,
        point.1 as f64,
        correction_degrees,
        pre_width as f64,
        pre_height as f64,
    );
    let render_x = (x * render_width as f64 / pre_width as f64)
        .round()
        .clamp(0.0, render_width as f64) as u32;
    let render_y = (y * render_height as f64 / pre_height as f64)
        .round()
        .clamp(0.0, render_height as f64) as u32;
    (render_x, render_y)
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn transform_ocr_geometry_to_render(
    geometry: &crate::types::OcrBoundingGeometry,
    correction_degrees: u16,
    pre_rotation_dimensions: (u32, u32),
    render_dimensions: (u32, u32),
) -> crate::types::OcrBoundingGeometry {
    match geometry {
        crate::types::OcrBoundingGeometry::Rectangle {
            left,
            top,
            width,
            height,
        } => {
            let first = transform_ocr_point_to_render(
                (*left, *top),
                correction_degrees,
                pre_rotation_dimensions,
                render_dimensions,
            );
            let second = transform_ocr_point_to_render(
                (left.saturating_add(*width), top.saturating_add(*height)),
                correction_degrees,
                pre_rotation_dimensions,
                render_dimensions,
            );
            let left = first.0.min(second.0);
            let top = first.1.min(second.1);
            crate::types::OcrBoundingGeometry::Rectangle {
                left,
                top,
                width: first.0.max(second.0).saturating_sub(left),
                height: first.1.max(second.1).saturating_sub(top),
            }
        }
        crate::types::OcrBoundingGeometry::Quadrilateral { points } => {
            let points = points.map(|point| {
                transform_ocr_point_to_render(point, correction_degrees, pre_rotation_dimensions, render_dimensions)
            });
            crate::types::OcrBoundingGeometry::Quadrilateral { points }
        }
    }
}

#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn transform_ocr_elements_to_render_space(
    elements: &[crate::types::OcrElement],
    metadata: &crate::types::Metadata,
    render_width: u32,
    render_height: u32,
) -> Vec<crate::types::OcrElement> {
    let Some((final_width, final_height)) = processed_ocr_layout_dimensions(metadata) else {
        return elements.to_vec();
    };
    let auto_rotated = metadata
        .additional
        .get(OCR_AUTO_ROTATED_METADATA_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let correction_degrees = resolved_ocr_correction_degrees(metadata);
    if auto_rotated && correction_degrees.is_none() {
        return elements.to_vec();
    }
    let correction_degrees = correction_degrees.unwrap_or(0);
    let pre_rotation_dimensions = if matches!(correction_degrees, 90 | 270) {
        (final_height, final_width)
    } else {
        (final_width, final_height)
    };
    elements
        .iter()
        .cloned()
        .map(|mut element| {
            element.geometry = transform_ocr_geometry_to_render(
                &element.geometry,
                correction_degrees,
                pre_rotation_dimensions,
                (render_width, render_height),
            );
            element
        })
        .collect()
}

/// Scale factor from OCR raster pixels to PDF points for one page, used to convert
/// pixel-derived font-size proxies into the same unit as the heading heuristic's
/// absolute-point constants (see `pdf::structure::adapters::resolve_ocr_font_size_pt`).
///
/// Requires the PDF document this OCR pass rendered from (`lazy_pdf_render_state`);
/// when that is unavailable — the caller supplied pre-rendered `images` directly, so
/// there is no `pdf_oxide::PdfDocument` in hand to read a MediaBox from — falls back
/// to `1.0` (pixels treated as points). That degrades the absolute-gap term of the
/// heading heuristic back toward today's behavior for that call path only; the
/// ratio-based term, which dominates in practice, is scale-invariant and unaffected.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ocr_points_per_pixel(
    #[cfg(feature = "pdf")] lazy_pdf_render_state: Option<&(pdf_oxide::PdfDocument, usize, Vec<u32>)>,
    page_idx: usize,
    page_height_px: u32,
) -> f32 {
    #[cfg(feature = "pdf")]
    {
        if page_height_px == 0 {
            return 1.0;
        }
        lazy_pdf_render_state
            .map(|(doc, _, _)| page_dimensions_pt(doc, page_idx).1 / page_height_px as f32)
            .filter(|scale| scale.is_finite() && *scale > 0.0)
            .unwrap_or(1.0)
    }
    #[cfg(not(feature = "pdf"))]
    {
        let _ = (page_idx, page_height_px);
        1.0
    }
}

#[cfg(all(any(feature = "ocr", feature = "ocr-wasm"), feature = "layout-detection"))]
fn assemble_ocr_page_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height: u32,
    detection: Option<&crate::layout::DetectionResult>,
    points_per_pixel: f32,
    // The page's PDF `/Rotate` value (0/90/180/270), or `0` when unknown (e.g. no
    // `pdf` feature). Threaded to the detached-list-marker reattachment passes
    // below so their baseline/indent comparisons run in the rotation-corrected
    // frame instead of the raw raster one (#760) -- see
    // `pdf::structure::pipeline::DetachedMarkerFrame`.
    page_rotation_degrees: u32,
) -> Vec<crate::pdf::structure::types::PdfParagraph> {
    // `doc`'s bbox AND ocr_geometry are still both raw OCR raster pixels at this point in
    // the pure-OCR route (the pixel -> point rescale runs later, in
    // `build_pipeline_ocr_page_document`), so one real points-per-pixel ratio scales both
    // font-size fallback branches identically. See
    // `pdf::structure::adapters::OcrFontSizeScale` for the mixed route, where that is not
    // true.
    let font_size_scale = crate::pdf::structure::adapters::OcrFontSizeScale::uniform(points_per_pixel);
    #[cfg(feature = "ocr")]
    if let Some(detection) = detection {
        let hints = super::layout_hints::detection_to_layout_hints_pixel_space(detection, page_height as f32);
        let mut paragraphs = crate::pdf::structure::adapters::ocr_doc_to_layout_paragraphs(
            doc,
            page_height,
            &hints,
            0.5,
            0.2,
            font_size_scale,
        );
        apply_ocr_text_list_fallback(&mut paragraphs);
        // #729: a bare marker no ML hint ever classified (`is_list_item` still
        // `false`) is invisible to `reattach_ocr_layout_list_markers` below, whose
        // marker-side test requires the opposite -- see
        // `adapters::reattach_detached_ocr_list_markers`'s doc comment. Runs first so
        // both passes only ever see markers still in their own precondition's state.
        crate::pdf::structure::adapters::reattach_detached_ocr_list_markers(&mut paragraphs, page_rotation_degrees);
        // #729: `regroup_layout_lines_by_element` (above, inside
        // `ocr_doc_to_layout_paragraphs`) isolates an ML-hinted list marker into its
        // own paragraph and never rejoins it to its body. Gated independently of
        // `pipeline::REATTACH_DETACHED_LIST_MARKERS` -- see
        // `adapters::REATTACH_OCR_LAYOUT_LIST_MARKERS`'s doc comment.
        crate::pdf::structure::adapters::reattach_ocr_layout_list_markers(&mut paragraphs, page_rotation_degrees);
        return paragraphs;
    }
    #[cfg(not(feature = "ocr"))]
    let _ = detection;

    crate::pdf::structure::adapters::ocr_doc_to_paragraphs(doc, page_height, font_size_scale)
}

/// Fill in `is_list_item` for paragraphs the OCR layout route left unclassified,
/// and OVERRIDE a layout classification that disagrees with an unambiguous text
/// list marker.
///
/// `ocr_doc_to_layout_paragraphs` (`crate::pdf::structure::adapters`) -- the OCR
/// counterpart of the native-PDF `finalize_paragraph`
/// (`crate::pdf::structure::pipeline`) -- derives `is_list_item` *exclusively* from
/// a layout-detection `ListItem` hint at >= 0.8 confidence
/// (`crate::pdf::structure::layout_classify::apply_hint_to_paragraph`). It never
/// falls back to a text-level marker check the way the native-PDF assembler always
/// does (`looks_like_list_item` runs unconditionally in `finalize_paragraph`,
/// independent of any layout hint). RT-DETR-style layout models commonly detect a
/// run of bulleted/numbered lines as one "Text" region rather than per-item
/// `ListItem` boxes, or miss/mislabel individual items outright -- including,
/// observed on `ordinance_2197_scanned.pdf`, classifying a numbered item as a
/// `Title`/`SectionHeader` (`## 8. Maximum height of structures: 50'`). When that
/// happens the item silently loses its list classification, and
/// `heuristically_restructured_ocr_pages`'s document-wide "already structured" gate
/// then refuses to re-derive it from segments, because that gate exists precisely
/// to protect a *correct* layout classification found elsewhere in the document
/// (see its doc comment). See #695.
///
/// This adds a text-marker pass directly onto the paragraphs the layout route
/// already built, so layout ADDS structure instead of silently dropping or
/// misclassifying what the text alone would have shown:
/// - A paragraph left with no classification at all (the common case) is filled in.
/// - A paragraph the layout route classified as a heading (`heading_level.is_some()`,
///   ordinarily from a `Title`/`SectionHeader` hint) is OVERRIDDEN when its text
///   unambiguously opens with a list marker: `heading_level` is cleared so the two
///   classifications never coexist (`assembly.rs`'s paragraph-to-element step checks
///   `heading_level` first and would otherwise render it as a heading, silently
///   discarding the list flag this function just set). This is safe specifically
///   because `looks_like_list_item` already rejects numbered SECTION headings via
///   `is_numbered_section_heading` ("1. INTRODUCTION", "3.2 Methods", "IV. Results"),
///   so text that passes the predicate is not a real heading in the first place --
///   the layout hint was wrong, not the text shape.
/// - A paragraph classified as code, a formula, or page furniture is NEVER touched:
///   those classifications are about the paragraph's *nature*, not a competing guess
///   at the same nature the way a `Text`/heading misclassification is, so a
///   coincidental marker-shaped prefix (a numbered code line, an OCR'd page-footer
///   digit) must not flip them.
/// - A paragraph the layout route already classified as a list item is left as-is.
///
/// Also reused, unmodified, as a *post-heuristic* fallback on the non-layout OCR
/// routes (`extract_mixed_ocr_native`, `run_ocr_pipeline`) for pages the document-global
/// heading/list heuristic (`heuristically_restructured_ocr_pages`) never classified --
/// either because it declined to run at all (`Plain` output, or another page in the same
/// document already carried an ML layout classification) or because it dropped a page
/// during the page split. Applying it there too (instead of only after ML layout hints)
/// closes the gap where a non-layout OCR document got zero list-item recovery of any kind
/// (#713). It is applied strictly *after* the heuristic has had its chance, never before:
/// pre-setting `is_list_item` earlier would itself flip `heuristically_restructured_ocr_pages`'s
/// "already structured" gate and skip heading detection for the whole document, trading a
/// list-item win for a heading-detection regression -- see that function's own doc comment.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn apply_ocr_text_list_fallback(paragraphs: &mut [crate::pdf::structure::types::PdfParagraph]) {
    for paragraph in paragraphs.iter_mut() {
        if paragraph.is_list_item || paragraph.is_code_block || paragraph.is_formula || paragraph.is_page_furniture {
            continue;
        }
        if crate::pdf::structure::pipeline::looks_like_list_item(paragraph.text.trim()) {
            paragraph.is_list_item = true;
            paragraph.heading_level = None;
            paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::ListItem);
        }
    }
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn fill_unstructured_ocr_pages(
    page_paragraphs: &mut [Option<Vec<crate::pdf::structure::types::PdfParagraph>>],
    page_texts: &[String],
) {
    for (page_index, paragraphs) in page_paragraphs.iter_mut().enumerate() {
        if paragraphs.as_ref().is_none_or(Vec::is_empty) {
            let fallback = crate::pdf::structure::adapters::ocr_text_to_paragraphs(&page_texts[page_index]);
            if !fallback.is_empty() {
                *paragraphs = Some(fallback);
            }
        }
    }
}

/// Run the document-global heading/list heuristic
/// (`pdf::structure::extract_document_structure_from_segments`, the same font-clustering
/// pass the native pdf_oxide path uses) over already-built OCR paragraphs.
///
/// The heuristic is document-global: `build_heading_map` clusters font sizes across
/// every page, and `sparse_multi_page_heading_map` needs at least two pages in hand.
/// `ocr_doc_to_paragraphs` / `ocr_doc_to_layout_paragraphs` build paragraphs one OCR
/// page at a time as OCR runs, so they cannot host this pass -- it can only run once
/// every page's paragraphs exist, i.e. here, after the whole document has been OCR'd.
///
/// Returns `None` (leaving the caller's pre-existing, unstructured assembly in place)
/// when:
/// - `config.output_format` is [`OutputFormat::Plain`]: plain-text output must stay
///   byte-identical to before this heuristic existed. The heuristic only ever changes
///   `heading_level` / `is_list_item`, which downstream assembly turns into different
///   `ElementKind`s -- never touched for `Plain`.
/// - Any paragraph already carries a `heading_level` or `is_list_item` set by ML
///   layout detection (`ocr_doc_to_layout_paragraphs`). That path already recovers
///   structure (measured 13/12/13 headings on the reference fixture); re-deriving
///   structure from bare segments would discard that classification, not add to it.
/// - The heuristic itself returns no elements, or errors (logged, not propagated: the
///   caller's existing unstructured assembly is always a safe fallback here).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn heuristically_restructured_ocr_pages(
    pages: &[Vec<crate::pdf::structure::types::PdfParagraph>],
    collected_tables: &[crate::types::Table],
    config: &ExtractionConfig,
) -> Option<crate::types::internal::InternalDocument> {
    if config.output_format == crate::core::config::OutputFormat::Plain {
        return None;
    }
    let already_structured = pages
        .iter()
        .flatten()
        .any(|paragraph| paragraph.heading_level.is_some() || paragraph.is_list_item);
    if already_structured {
        return None;
    }

    let all_page_segments = crate::pdf::structure::adapters::segments_from_ocr_pages(pages);
    let k_clusters = config
        .pdf_options
        .as_ref()
        .and_then(|opts| opts.hierarchy.as_ref())
        .map(|hierarchy| hierarchy.k_clusters)
        .unwrap_or(4);

    let result = crate::pdf::structure::extract_document_structure_from_segments(
        all_page_segments,
        crate::pdf::structure::SegmentStructureConfig {
            k_clusters,
            tables: collected_tables,
            outline_entries: &[],
            strip_repeating_text: false,
            include_headers: false,
            include_footers: false,
            include_footnotes: false,
            used_structure_tree: false,
            image_positions: &[],
            images: None,
            inject_placeholders: false,
            layout_hints: None,
            allow_single_column: true,
            cancel_token: config.cancel_token.as_ref(),
            #[cfg(feature = "layout-detection")]
            layout_images: None,
            #[cfg(feature = "layout-detection")]
            layout_results: None,
            #[cfg(feature = "layout-detection")]
            table_model: crate::core::config::layout::TableModel::default(),
            #[cfg(feature = "layout-detection")]
            table_overlap_preference: crate::core::config::layout::TableOverlapPreference::default(),
            #[cfg(feature = "layout-detection")]
            acceleration: None,
            #[cfg(feature = "layout-detection")]
            session_thread_budget: 0,
        },
    );

    match result {
        Ok(doc) if !doc.elements.is_empty() => Some(doc),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "OCR document-level heading/list heuristic failed; falling back to unstructured OCR paragraphs"
            );
            None
        }
    }
}

/// Split the single, document-wide [`crate::types::internal::InternalDocument`]
/// [`heuristically_restructured_ocr_pages`] produced back into one per-page document per
/// real OCR'd page number, for [`extract_mixed_ocr_native`].
///
/// That function's per-page `structured_ocr_pages` map is what
/// [`merge_structured_ocr_pages_into_internal_document`] uses to splice each OCR'd page's
/// structure back into the surrounding native document at the right position -- it needs
/// one document per page, not one document spanning every OCR'd page. The combined
/// document's elements/tables/images all carry a real page number (`element.page`,
/// `table.page_number`, `image.page_number`) because the caller pads its `pages` argument
/// to the heuristic out to the *document's* full page count and only populates the slots
/// for actually-OCR'd pages (see the caller), so `extract_document_structure_from_segments`
/// numbers pages 1:1 with real page numbers, not with position in a filtered subset.
///
/// `ElementKind::Table`/`ElementKind::Image` index into the *combined* document's
/// `tables`/`images` vecs; each per-page document gets its own 0-based vecs, so those
/// indices are remapped here. Relationships (e.g. caption associations) are dropped: they
/// index into the combined document's element list, which no longer exists in one piece
/// after this split, matching the pre-existing per-page builders
/// ([`build_mixed_ocr_page_document`], [`build_pipeline_ocr_page_document`]), neither of
/// which carries cross-page relationships either.
///
/// A page in `ocr_page_numbers` that ended up with no elements, tables, or images after
/// the split (e.g. the heuristic dropped an empty page) has no entry in the returned map,
/// mirroring the `None`-means-"nothing structured" contract the caller's pre-existing
/// per-page builders already use.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn split_document_global_ocr_structure_by_page(
    doc: crate::types::internal::InternalDocument,
    ocr_page_numbers: &[u32],
) -> ahash::AHashMap<u32, crate::types::internal::InternalDocument> {
    use crate::types::internal::ElementKind;

    let mut elements_by_page: ahash::AHashMap<u32, Vec<crate::types::internal::InternalElement>> =
        ahash::AHashMap::new();
    for element in doc.elements {
        if matches!(element.kind, ElementKind::PageBreak) {
            continue;
        }
        if let Some(page) = element.page {
            elements_by_page.entry(page).or_default().push(element);
        }
    }

    let tables = doc.tables;
    let images = doc.images;

    let mut result = ahash::AHashMap::with_capacity(ocr_page_numbers.len());
    for &page_number in ocr_page_numbers {
        let Some(mut elements) = elements_by_page.remove(&page_number) else {
            continue;
        };

        let mut page_tables = Vec::new();
        let mut page_images = Vec::new();
        for element in &mut elements {
            match &mut element.kind {
                ElementKind::Table { table_index } => {
                    if let Some(table) = tables.get(*table_index as usize) {
                        let new_index = page_tables.len() as u32;
                        page_tables.push(table.clone());
                        *table_index = new_index;
                    }
                }
                ElementKind::Image { image_index } => {
                    if let Some(image) = images.get(*image_index as usize) {
                        let new_index = page_images.len() as u32;
                        page_images.push(image.clone());
                        *image_index = new_index;
                    }
                }
                _ => {}
            }
        }

        if elements.is_empty() && page_tables.is_empty() && page_images.is_empty() {
            continue;
        }

        let mut page_doc = crate::types::internal::InternalDocument::new("pdf");
        page_doc.elements = elements;
        page_doc.tables = page_tables;
        page_doc.images = page_images;
        result.insert(page_number, page_doc);
    }
    result
}

/// Convert a TATR-recognized table into the public [`crate::types::Table`],
/// carrying over its `detection_bbox` and assigning a deterministic `table_id`.
///
/// `table_index` is the table's 0-based position in the document's push order
/// (see the caller), so the id is `"table-{table_index + 1}"` — never derived
/// from randomness or wall-clock time, so the same input document always
/// produces the same id. See [`crate::types::Table::table_id`] for the shared
/// scheme doc.
#[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
fn recognized_table_to_public_table(
    recognized: &crate::RecognizedTable,
    page_number: u32,
    table_index: usize,
) -> crate::types::Table {
    crate::types::Table {
        cells: recognized.cells.clone(),
        markdown: recognized.markdown.clone(),
        page_number,
        bounding_box: Some(crate::types::BoundingBox {
            x0: recognized.detection_bbox.x1 as f64,
            y0: recognized.detection_bbox.y1 as f64,
            x1: recognized.detection_bbox.x2 as f64,
            y1: recognized.detection_bbox.y2 as f64,
        }),
        table_id: Some(format!("table-{}", table_index + 1)),
        columns: recognized.cells.first().cloned(),
    }
}

/// Extract text from PDF using OCR on pre-rendered page images.
///
/// When `layout_detections` are provided (pixel-space, from the same images), uses
/// layout-aware markdown assembly for structured output. Otherwise, when
/// `config.output_format` is not [`OutputFormat::Plain`], structure (headings, list
/// items) is instead recovered document-wide by the same font-clustering heuristic
/// the native pdf_oxide path uses
/// (`pdf::structure::pipeline::extract_document_structure_from_segments`), fed from
/// segments harvested out of the per-page OCR paragraphs
/// (`pdf::structure::adapters::segments_from_ocr_pages`). Under `Plain`, or when that
/// heuristic yields nothing, falls back to plain OCR text concatenation.
///
/// # Arguments
///
/// * `images` - Pre-rendered page images (shared with layout detection)
/// * `layout_detections` - Optional pixel-space layout detections per page
/// * `config` - Extraction configuration including OCR settings
///
/// # Returns
///
/// Concatenated text from all pages, with markdown structure when layout is available
///
/// Thin, signature-preserving wrapper over [`extract_with_ocr_for_page`] with no page
/// rotation override (`0`), so every pre-existing caller keeps today's behavior: content-based
/// per-page auto-detection when `content` is available and index-aligned to `images`, or no
/// rotation correction at all otherwise.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) async fn extract_with_ocr(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    Option<f64>,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
)> {
    let (text, mean_conf, tables, elements, doc, usage, page_texts, rasters, formulas, _raw_page_paragraphs) =
        Box::pin(extract_with_ocr_for_page(
            content,
            images,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            config,
            path,
            0,
            false,
            None,
        ))
        .await?;
    Ok((
        text, mean_conf, tables, elements, doc, usage, page_texts, rasters, formulas,
    ))
}

/// Same as [`extract_with_ocr`], but `page_rotation_override` -- when non-zero -- is used as
/// the known `/Rotate` value for every image in `images` instead of this function's own
/// content-based per-page lookup (`lazy_pdf_render_state` / `external_image_page_rotations`).
///
/// Needed by a caller that hands in a single image detached from its original page index
/// (currently only [`run_ocr_pipeline_for_page`]'s per-stage runs), where an index-based
/// content lookup would silently resolve the wrong page's rotation (or none at all) for any
/// page but the document's first. `0` defers entirely to the existing per-page
/// auto-detection -- see [`extract_with_ocr`], the public entry point every other caller uses.
///
/// `skip_document_global_heuristic` -- when `true`, skips the internal
/// [`heuristically_restructured_ocr_pages`] call and always uses the plain,
/// unstructured `assemble_internal_document` fallback for the returned document instead.
/// Needed by [`extract_mixed_ocr_native`]'s per-page pipeline route (#665/#1423-followup):
/// that caller drives this function with exactly one image per call (one PDF page detached
/// from the rest of the document), so the heuristic -- which clusters font sizes and needs
/// several pages in hand to be useful -- would only ever see a single page here and either
/// do nothing or, worse, mark that page's paragraphs "already structured" and block the
/// caller's own *document-wide* pass over every OCR'd page from running at all. The extra
/// tuple element this function returns (bare, unclassified per-page paragraphs) is what lets
/// that caller run the heuristic itself, once, over the whole document.
///
/// `points_per_pixel_override` -- when `Some`, used in place of this function's own
/// [`ocr_points_per_pixel`] lookup for every image in `images`. That lookup resolves a page's
/// pixel -> point scale from `lazy_pdf_render_state`, derived from `content` and indexed by
/// this function's own 0-based loop position over `images` -- exactly the "single detached
/// image is not really page 0" problem `page_rotation_override` already exists to solve, so a
/// caller driving one detached image per call (currently only
/// `extract_mixed_ocr_native`'s per-page pipeline route) needs the same override here: without
/// it, `content: None` makes the lookup fall back to `1.0` (pixels treated as points), silently
/// defeating the document-global heading heuristic's absolute-point font-gap comparisons.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[allow(clippy::too_many_arguments)]
async fn extract_with_ocr_for_page(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    path: Option<&std::path::Path>,
    page_rotation_override: u32,
    skip_document_global_heuristic: bool,
    points_per_pixel_override: Option<f32>,
) -> crate::Result<(
    String,
    Option<f64>,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
)> {
    use crate::plugins::registry::get_ocr_backend_registry;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    use std::io::Cursor;

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let base_ocr_config = config.ocr.as_ref().unwrap_or(&default_ocr_config);

    let accel_ocr_config;
    let base_ocr_config = if base_ocr_config.acceleration.is_none() && config.acceleration.is_some() {
        accel_ocr_config = {
            let mut c = base_ocr_config.clone();
            c.acceleration = config.acceleration.clone();
            c
        };
        &accel_ocr_config
    } else {
        base_ocr_config
    };

    let backend = {
        let registry = get_ocr_backend_registry();
        let registry = registry.read();
        registry.get(&base_ocr_config.backend)?
    };
    // Only `ConfidenceSemantics::Legibility` backends report a `mean_text_conf` whose scale
    // means legibility; anything else (`Uncalibrated`, `None`) must never be turned into a
    // normalized confidence, or a page-rejection gate downstream would compare it against an
    // absolute floor it was never calibrated against (see `resolve_confidence_semantics`).
    let backend_confidence_scale = match backend.confidence_semantics() {
        crate::plugins::ConfidenceSemantics::Legibility { scale_max } if scale_max > 0.0 => Some(scale_max),
        _ => None,
    };
    // Only meaningful with the `pdf` feature: without it there is no `/Rotate` to correct for
    // (`page_rotation_degrees` is always `0` below), so nothing reads this in that build.
    #[cfg(feature = "pdf")]
    let orientation_handling = backend.page_orientation_handling();

    let structured_ocr_config;
    let ocr_config = {
        let cfg = ensure_elements_enabled(base_ocr_config);
        #[cfg(all(feature = "ocr", feature = "layout-detection"))]
        let cfg = if layout_detections.is_some() || backend.emits_structured_markdown() {
            inject_layout_config_to_backend(&cfg, config)
        } else {
            cfg
        };
        structured_ocr_config = cfg;
        &structured_ocr_config
    };

    #[cfg(not(feature = "layout-detection"))]
    let supports_doc = backend.supports_document_processing();
    #[cfg(feature = "layout-detection")]
    let supports_doc = backend.supports_document_processing() && layout_detections.is_none();

    let use_document_processing = supports_doc && path.is_some();

    if let Some(doc_path) = path
        && use_document_processing
    {
        tracing::debug!(backend = %ocr_config.backend, "Using document-level OCR processing");
        let result = backend.process_document(doc_path, ocr_config).await?;
        let mean_conf = backend_confidence_scale.and_then(|scale_max| {
            result
                .metadata
                .additional
                .get("mean_text_conf")
                .and_then(|v| v.as_f64())
                .map(|v| v / scale_max)
        });
        let backend_elements = result.ocr_elements.unwrap_or_default();
        let ocr_elements = filter_public_ocr_elements(&backend_elements, base_ocr_config);
        let llm_usage = result.llm_usage.unwrap_or_default();
        let formulas = result.formulas;
        let page_texts = if let Some(pages) = result.pages {
            pages.into_iter().map(|p| p.content).collect()
        } else {
            vec![result.content.clone()]
        };
        return Ok((
            result.content,
            mean_conf,
            Vec::new(),
            ocr_elements,
            None,
            llm_usage,
            page_texts,
            None,
            formulas,
            // The document-level backend route returns whole-document content, not per-page
            // paragraph geometry, so there is nothing for the document-global heuristic to
            // cluster over here.
            Vec::new(),
        ));
    }
    let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
    let mut captured_rasters: Vec<crate::types::ExtractedImage> = Vec::new();

    #[cfg(feature = "pdf")]
    let lazy_pdf_render_state = if !use_document_processing && images.is_none() {
        content.map(open_pdf_for_full_ocr).transpose()?
    } else {
        None
    };
    #[cfg(feature = "pdf")]
    let lazy_pdf_page_count = lazy_pdf_render_state
        .as_ref()
        .map_or(0, |(_, page_count, _)| *page_count);
    #[cfg(not(feature = "pdf"))]
    let lazy_pdf_page_count = 0;

    // rayon's work-stealing pool needs OS threads; wasm32 has none, so the parallel encode
    // paths below fall back to sequential `.iter()` there. Gate the import to match. ~keep
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use rayon::prelude::*;
    use std::sync::Arc;
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    use tokio::task::JoinSet;

    let configured_batch_size = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());

    let batch_size = if images.is_none() {
        adapt_batch_size_to_memory(configured_batch_size, content.map(|b| b.len()).unwrap_or(0))
    } else {
        configured_batch_size
    };

    if batch_size < configured_batch_size {
        tracing::info!(
            configured = configured_batch_size,
            adapted = batch_size,
            "Reduced OCR batch size to fit available memory"
        );
    }

    let mut ocr_config_owned = ocr_config.clone();
    ocr_config_owned.acceleration = config.acceleration.clone();
    let total_pages = if let Some(imgs) = images {
        imgs.len()
    } else {
        lazy_pdf_page_count
    };

    // The layout-detection route hands in pre-rendered `images`, so `lazy_pdf_render_state`
    // above is never populated (it's gated on `images.is_none()`) and the page-rotation
    // lookups below would always fall back to their `unwrap_or(0)` default. When the
    // original PDF bytes are still available via `content`, read the same lightweight
    // `/Rotate` lookup the full-page and per-page routes use so this route's calls to
    // `ocr_config_with_page_rotation_hint` get a real hint too (see #530 / 972d2269f7).
    // `content` is `None` on some image-only callers (e.g. the image extractor's own OCR
    // path with no source PDF); there is genuinely no rotation to read in that case.
    #[cfg(feature = "pdf")]
    let external_image_page_rotations: Option<Vec<u32>> = if images.is_some() {
        content.map(|c| crate::pdf::render::get_page_rotations_from_bytes(c, total_pages))
    } else {
        None
    };

    let mut page_texts = vec![String::new(); total_pages];
    let mut all_page_paragraphs: Vec<Option<Vec<crate::pdf::structure::types::PdfParagraph>>> = vec![None; total_pages];
    #[allow(unused_mut)]
    let mut collected_tables: Vec<crate::types::Table> = Vec::new();
    let mut all_ocr_elements: Vec<crate::types::OcrElement> = Vec::new();
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut conf_sum: f64 = 0.0;
    let mut conf_count: usize = 0;
    // Warnings from the force_ocr image-XObject fallback (#1355): a page rendered
    // blank by pdf_oxide but carrying image XObjects the renderer couldn't paint.
    #[cfg(feature = "pdf")]
    let mut image_fallback_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    // #1444: a backend failure on one page used to propagate with `?`, aborting the whole
    // extraction and — crucially — never reaching the image-XObject fallback below, which is
    // exactly the recovery such a page needs. Failures are captured per page instead: the
    // page degrades to empty text (so the blank-page fallback fires for it), the failure is
    // surfaced as a warning, and only a document where *every* page failed *and* nothing was
    // recovered still returns an error.
    let mut page_backend_errors: Vec<(usize, String)> = Vec::new();
    let mut page_failure_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    // Opened on first blank page only; see `fallback_render_document`.
    #[cfg(feature = "pdf")]
    let mut fallback_pdf_state: Option<Option<pdf_oxide::PdfDocument>> = None;

    // Judged per page just before the OCR text is accepted, so a drawing page contributes
    // nothing instead of contributing invented words. See `is_ocr_recognition_noise`.
    let ocr_output_thresholds = base_ocr_config.quality_thresholds.clone().unwrap_or_default();
    let mut recognition_noise_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    #[cfg(feature = "layout-detection")]
    let mut tatr_model = if layout_detections.is_some() {
        crate::layout::take_or_create_tatr(
            config.resolved_layout_acceleration(),
            crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref()),
        )
    } else {
        None
    };

    for batch_start in (0..total_pages).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(total_pages);

        #[allow(unused_variables)]
        let (batch_slice, encoded_batch) = if let Some(imgs) = images {
            let slice: Cow<'_, [image::DynamicImage]> = Cow::Borrowed(&imgs[batch_start..batch_end]);
            #[allow(clippy::type_complexity)]
            #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
            let encoded: crate::Result<Vec<(usize, Arc<Vec<u8>>, u32, u32)>> = slice
                .par_iter()
                .enumerate()
                .map(|(offset, image)| {
                    let page_idx = batch_start + offset;
                    let rgb_image = image.to_rgb8();
                    let (width, height) = rgb_image.dimensions();
                    let mut image_bytes = Cursor::new(Vec::new());
                    let encoder = PngEncoder::new(&mut image_bytes);
                    encoder
                        .write_image(&rgb_image, width, height, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode image: {}", e),
                            source: None,
                        })?;
                    Ok((page_idx, Arc::new(image_bytes.into_inner()), width, height))
                })
                .collect();
            #[allow(clippy::type_complexity)]
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            let encoded: crate::Result<Vec<(usize, Arc<Vec<u8>>, u32, u32)>> = slice
                .iter()
                .enumerate()
                .map(|(offset, image)| {
                    let page_idx = batch_start + offset;
                    let rgb_image = image.to_rgb8();
                    let (width, height) = rgb_image.dimensions();
                    let mut image_bytes = Cursor::new(Vec::new());
                    let encoder = PngEncoder::new(&mut image_bytes);
                    encoder
                        .write_image(&rgb_image, width, height, image::ColorType::Rgb8.into())
                        .map_err(|e| crate::XbergError::Parsing {
                            message: format!("Failed to encode image: {}", e),
                            source: None,
                        })?;
                    Ok((page_idx, Arc::new(image_bytes.into_inner()), width, height))
                })
                .collect();
            (Some(slice), encoded?)
        } else {
            #[cfg(feature = "pdf")]
            let encoded = {
                let (doc, _, page_rotations) =
                    lazy_pdf_render_state
                        .as_ref()
                        .ok_or_else(|| crate::XbergError::Parsing {
                            message: "PDF content is required for OCR rendering but was not provided".to_string(),
                            source: None,
                        })?;
                render_full_pdf_ocr_batch(doc, page_rotations, batch_start..batch_end)?
            };
            #[cfg(not(feature = "pdf"))]
            let encoded: Vec<(usize, Arc<Vec<u8>>, u32, u32)> = Vec::new();
            (None::<Cow<'_, [image::DynamicImage]>>, encoded)
        };

        let batch_count = encoded_batch.len();
        let mut batch_ocr_results: Vec<Option<crate::types::ExtractedDocument>> = vec![None; batch_count];
        // Backend error text for a page whose OCR call failed, if any (#1444). The page's
        // entry in `batch_ocr_results` is an empty document in that case, so the blank-page
        // image-XObject fallback below gets its chance before the failure is reported.
        let mut batch_page_errors: Vec<Option<String>> = vec![None; batch_count];
        // (correction_degrees, upright_width, upright_height) applied by
        // `upright_raster_for_backend` before this page's backend call, so the matching
        // `undo_upright_raster_correction` below maps the result back correctly (#643).
        let mut batch_upright_correction: Vec<(u32, u32, u32)> = vec![(0, 0, 0); batch_count];

        // See the sibling JoinSet block above: `Send` futures aren't available on wasm32. ~keep
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        {
            let mut join_set: JoinSet<(usize, u32, u32, u32, crate::Result<crate::types::ExtractedDocument>)> =
                JoinSet::new();
            for (page_idx, image_data, width, height) in &encoded_batch {
                let backend_clone = std::sync::Arc::clone(&backend);
                #[cfg(feature = "pdf")]
                let page_rotation_degrees = if page_rotation_override != 0 {
                    page_rotation_override
                } else {
                    lazy_pdf_render_state
                        .as_ref()
                        .and_then(|(_, _, rotations)| rotations.get(*page_idx))
                        .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(*page_idx)))
                        .copied()
                        .unwrap_or(0)
                };
                #[cfg(not(feature = "pdf"))]
                let page_rotation_degrees: u32 = 0;
                let config_clone =
                    ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees).into_owned();
                // No PDF `/Rotate` is ever known without the `pdf` feature (`page_rotation_degrees`
                // is always `0` above in that build), so there is nothing to correct upright.
                #[cfg(feature = "pdf")]
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    image_data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                )?;
                #[cfg(not(feature = "pdf"))]
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    (Arc::clone(image_data), *width, *height, 0u32);
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(upright_data, &config_clone).await;
                    (idx, correction_degrees, upright_width, upright_height, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, correction_degrees, upright_width, upright_height, ocr_result) =
                    join_result.map_err(|e| crate::XbergError::Plugin {
                        message: format!("OCR task panicked: {}", e),
                        plugin_name: "ocr".to_string(),
                    })?;
                batch_upright_correction[page_idx - batch_start] = (correction_degrees, upright_width, upright_height);
                match ocr_result {
                    Ok(document) => batch_ocr_results[page_idx - batch_start] = Some(document),
                    Err(error) => {
                        tracing::warn!(page = page_idx + 1, error = %error, "OCR backend failed for page");
                        batch_page_errors[page_idx - batch_start] = Some(error.to_string());
                        batch_ocr_results[page_idx - batch_start] = Some(crate::types::ExtractedDocument::default());
                    }
                }
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, image_data, width, height) in &encoded_batch {
                #[cfg(feature = "pdf")]
                let page_rotation_degrees = if page_rotation_override != 0 {
                    page_rotation_override
                } else {
                    lazy_pdf_render_state
                        .as_ref()
                        .and_then(|(_, _, rotations)| rotations.get(*page_idx))
                        .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(*page_idx)))
                        .copied()
                        .unwrap_or(0)
                };
                #[cfg(not(feature = "pdf"))]
                let page_rotation_degrees: u32 = 0;
                let config_for_page = ocr_config_with_page_rotation_hint(&ocr_config_owned, page_rotation_degrees);
                #[cfg(feature = "pdf")]
                let (upright_data, upright_width, upright_height, correction_degrees) = upright_raster_for_backend(
                    image_data,
                    *width,
                    *height,
                    page_rotation_degrees,
                    orientation_handling,
                )?;
                #[cfg(not(feature = "pdf"))]
                let (upright_data, upright_width, upright_height, correction_degrees) =
                    (Arc::clone(image_data), *width, *height, 0u32);
                let ocr_result = backend
                    .process_image(upright_data.as_slice(), config_for_page.as_ref())
                    .await;
                batch_upright_correction[page_idx - batch_start] = (correction_degrees, upright_width, upright_height);
                match ocr_result {
                    Ok(document) => batch_ocr_results[page_idx - batch_start] = Some(document),
                    Err(error) => {
                        tracing::warn!(page = page_idx + 1, error = %error, "OCR backend failed for page");
                        batch_page_errors[page_idx - batch_start] = Some(error.to_string());
                        batch_ocr_results[page_idx - batch_start] = Some(crate::types::ExtractedDocument::default());
                    }
                }
            }
        }

        for offset in 0..batch_count {
            let page_idx = batch_start + offset;
            let mut ocr_result = batch_ocr_results[offset].take().expect("OCR result missing for page");
            #[cfg(feature = "pdf")]
            {
                let (correction_degrees, upright_width, upright_height) = batch_upright_correction[offset];
                undo_upright_raster_correction(&mut ocr_result, correction_degrees, upright_width, upright_height);
            }
            #[cfg(feature = "layout-detection")]
            let _height = encoded_batch[offset].3;

            if let Some(conf_val) = ocr_result
                .metadata
                .additional
                .get("mean_text_conf")
                .and_then(|v| v.as_i64())
            {
                conf_sum += conf_val as f64;
                conf_count += 1;
            }

            if let Some(usage) = ocr_result.llm_usage.take() {
                accumulated_llm_usage.extend(usage);
            }

            if let Some(ref mut elems) = ocr_result.ocr_elements {
                for elem in elems.iter_mut() {
                    elem.page_number = (page_idx + 1) as u32;
                }
                all_ocr_elements.extend(filter_public_ocr_elements(elems, base_ocr_config));
            }

            for mut formula in ocr_result.formulas {
                formula.page = Some((page_idx + 1) as u32);
                #[cfg(feature = "pdf")]
                if let Some((doc, _, _)) = lazy_pdf_render_state.as_ref() {
                    let (w, h) = (encoded_batch[offset].2, encoded_batch[offset].3);
                    formula_bbox_to_page_points(&mut formula, doc, page_idx, Some(&ocr_result.metadata), w, h);
                }
                accumulated_formulas.push(formula);
            }

            // force_ocr image-XObject fallback (#1355): pdf_oxide can catch an
            // image-decode error internally and substitute a blank white bitmap for
            // the whole-page render, so the page comes back from OCR as blank with no
            // indication anything was wrong. When that happens and the page actually
            // carries image XObjects, retry OCR directly on the embedded image bytes
            // (decoded pixels re-encoded to PNG, or the raw JPEG/JP2 stream) and always
            // surface a warning so the silent drop becomes visible.
            //
            // #1444 widened when this runs: a page whose backend call *failed* arrives here
            // as an empty document (see `batch_page_errors`), and a page the backend merely
            // *described* as blank is caught by the ink probe in
            // `page_needs_xobject_fallback` -- both used to sail past this block.
            #[cfg(feature = "pdf")]
            if page_needs_xobject_fallback(&ocr_result.content, encoded_batch[offset].1.as_slice()) {
                // The layout-detection route hands in pre-rendered `images`, which leaves
                // `lazy_pdf_render_state` unopened; that used to disable this fallback
                // entirely for exactly the scanned documents most likely to need it. The
                // fallback needs nothing from the render state but the page's XObject
                // table, so it opens its own handle rather than perturbing the rotation and
                // points-per-pixel lookups that state is indexed for (#1444).
                let render_doc = match lazy_pdf_render_state.as_ref() {
                    Some((doc, _, _)) => Some(doc),
                    None => fallback_render_document(&mut fallback_pdf_state, content),
                };
                if let Some(render_doc) = render_doc
                    && let Some(recovery) =
                        recover_page_text_from_image_xobjects(&backend, render_doc, page_idx, &ocr_config_owned).await
                {
                    if !recovery.text.is_empty() {
                        ocr_result.content = recovery.text;
                    }
                    if capture_rasters {
                        captured_rasters.extend(recovery.images);
                    }
                    image_fallback_warnings.push(xobject_fallback_warning(page_idx, recovery.attempted));
                }
            }

            // Report a page whose backend call failed, now that the fallback above has had
            // its chance to recover it (#1444). Either way the failure is visible: a page
            // that vanishes silently is the defect this replaces.
            if let Some(error) = batch_page_errors[offset].take() {
                let recovered = !ocr_result.content.trim().is_empty();
                page_failure_warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("ocr"),
                    message: std::borrow::Cow::Owned(if recovered {
                        format!(
                            "OCR of page {} failed ({error}); its text was recovered from the page's \
                             embedded image XObjects instead.",
                            page_idx + 1
                        )
                    } else {
                        format!(
                            "OCR of page {} failed and could not be recovered: {error}",
                            page_idx + 1
                        )
                    }),
                });
                page_backend_errors.push((page_idx, error));
            }

            #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
            if ocr_result.ocr_internal_document.is_some()
                || ocr_result
                    .ocr_elements
                    .as_ref()
                    .is_some_and(|elements| !elements.is_empty())
            {
                let elements = ocr_result.ocr_elements.as_deref().unwrap_or_default();
                let detection = layout_detections.and_then(|detections| detections.get(page_idx));

                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let render_scaled_detection =
                    detection.map(|det| scale_detection_to_dimensions(det, ocr_render_width, ocr_render_height));
                let (_, ocr_layout_height) =
                    resolved_ocr_layout_dimensions(&ocr_result.metadata, ocr_render_width, ocr_render_height);
                let points_per_pixel = points_per_pixel_override.unwrap_or_else(|| {
                    ocr_points_per_pixel(
                        #[cfg(feature = "pdf")]
                        lazy_pdf_render_state.as_ref(),
                        page_idx,
                        ocr_layout_height,
                    )
                });
                let ocr_scaled_detection = detection.map(|det| {
                    scale_detection_to_ocr_coordinates(det, &ocr_result.metadata, ocr_render_width, ocr_render_height)
                });
                let render_ocr_elements = transform_ocr_elements_to_render_space(
                    elements,
                    &ocr_result.metadata,
                    ocr_render_width,
                    ocr_render_height,
                );

                let recognized_tables = match (render_scaled_detection.as_ref(), tatr_model.as_mut()) {
                    (Some(scaled_det), Some(model)) => {
                        let rgb = if let Some(ref slice) = batch_slice {
                            slice[offset].to_rgb8()
                        } else {
                            let png_data = &encoded_batch[offset].1;
                            let decoded =
                                image::load_from_memory(png_data).map_err(|e| crate::XbergError::Parsing {
                                    message: format!("Failed to decode PNG for TATR: {}", e),
                                    source: None,
                                })?;
                            decoded.to_rgb8()
                        };
                        crate::ocr::layout_assembly::recognize_page_tables(
                            &rgb,
                            scaled_det,
                            &render_ocr_elements,
                            model,
                        )
                    }
                    _ => Vec::new(),
                };

                for rt in &recognized_tables {
                    if !rt.markdown.is_empty() {
                        // The id is this table's 1-based position in `collected_tables`;
                        // pages are processed strictly in increasing `page_idx` order
                        // above, so push order is deterministic document order. ~keep
                        let table_index = collected_tables.len();
                        collected_tables.push(recognized_table_to_public_table(rt, (page_idx + 1) as u32, table_index));
                    }
                }

                if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                    // Same lookup as the per-page `page_rotation_degrees` computed above for
                    // the backend call (#760) -- that one is scoped to the join_set/backend
                    // loop and does not survive to here, so it is recomputed identically:
                    // an explicit override wins, otherwise the PDF's own known `/Rotate` for
                    // this page (falling back to the image-derived rotation when the render
                    // state was never opened), defaulting to `0` when nothing is known.
                    #[cfg(feature = "pdf")]
                    let page_rotation_degrees = if page_rotation_override != 0 {
                        page_rotation_override
                    } else {
                        lazy_pdf_render_state
                            .as_ref()
                            .and_then(|(_, _, rotations)| rotations.get(page_idx))
                            .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(page_idx)))
                            .copied()
                            .unwrap_or(0)
                    };
                    #[cfg(not(feature = "pdf"))]
                    let page_rotation_degrees: u32 = 0;
                    let paragraphs = assemble_ocr_page_paragraphs(
                        ocr_doc,
                        ocr_layout_height,
                        ocr_scaled_detection.as_ref(),
                        points_per_pixel,
                        page_rotation_degrees,
                    );

                    tracing::debug!(
                        page = page_idx + 1,
                        paragraphs = paragraphs.len(),
                        raw_content_len = ocr_result.content.len(),
                        "OCR page layout classification complete"
                    );

                    all_page_paragraphs[page_idx] = Some(paragraphs);
                }

                if capture_rasters {
                    let (_, png_arc, w, h) = &encoded_batch[offset];
                    let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                    captured_rasters.push(build_page_raster_image(page_idx, png_bytes, *w, *h));
                }
                page_texts[page_idx] = accept_or_reject_ocr_page(
                    page_idx,
                    ocr_result.content,
                    &ocr_output_thresholds,
                    &mut recognition_noise_warnings,
                );
                continue;
            }

            #[cfg(not(feature = "layout-detection"))]
            if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let (_, ocr_layout_height) =
                    resolved_ocr_layout_dimensions(&ocr_result.metadata, ocr_render_width, ocr_render_height);
                let points_per_pixel = points_per_pixel_override.unwrap_or_else(|| {
                    ocr_points_per_pixel(
                        #[cfg(feature = "pdf")]
                        lazy_pdf_render_state.as_ref(),
                        page_idx,
                        ocr_layout_height,
                    )
                });
                // `ocr_doc`'s bbox AND ocr_geometry are both still raw OCR raster pixels here
                // (same pure-OCR-route reasoning as `assemble_ocr_page_paragraphs` above).
                let font_size_scale = crate::pdf::structure::adapters::OcrFontSizeScale::uniform(points_per_pixel);
                let paragraphs =
                    crate::pdf::structure::adapters::ocr_doc_to_paragraphs(ocr_doc, ocr_layout_height, font_size_scale);
                all_page_paragraphs[page_idx] = Some(paragraphs);
            }

            let _ = page_idx;
            if capture_rasters {
                let (_, png_arc, w, h) = &encoded_batch[offset];
                let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                captured_rasters.push(build_page_raster_image(page_idx, png_bytes, *w, *h));
            }
            page_texts[page_idx] = accept_or_reject_ocr_page(
                page_idx,
                ocr_result.content,
                &ocr_output_thresholds,
                &mut recognition_noise_warnings,
            );
        }
    }

    #[cfg(feature = "layout-detection")]
    if let Some(model) = tatr_model.take() {
        crate::layout::return_tatr(model);
    }

    // Degrading a per-page failure to a warning must not turn a wholesale OCR failure into a
    // silently empty document: when every page failed *and* nothing was recovered from any
    // page's embedded images, there is no partial result to preserve, so report it (#1444).
    if !page_backend_errors.is_empty()
        && page_backend_errors.len() == total_pages
        && page_texts.iter().all(|text| text.trim().is_empty())
    {
        let (_, first_error) = &page_backend_errors[0];
        return Err(crate::XbergError::Plugin {
            message: format!(
                "OCR failed on all {total_pages} page(s) and no text could be recovered from the pages' \
                 embedded images; first failure: {first_error}"
            ),
            plugin_name: "ocr".to_string(),
        });
    }

    let mean_text_conf = match (conf_count > 0, backend_confidence_scale) {
        (true, Some(scale_max)) => Some((conf_sum / conf_count as f64) / scale_max),
        _ => None,
    };

    let page_marker_cfg = config.pages.as_ref().filter(|p| p.insert_page_markers);
    let mut result = String::new();
    for (i, text) in page_texts.iter().enumerate() {
        if let Some(cfg) = page_marker_cfg {
            let marker = cfg.marker_format.replace("{page_num}", &(i + 1).to_string());
            result.push_str(&marker);
        } else if i > 0 {
            result.push_str("\n\n");
        }
        result.push_str(text);
    }

    fill_unstructured_ocr_pages(&mut all_page_paragraphs, &page_texts);

    let (ocr_doc, raw_page_paragraphs) = {
        let has_structured = all_page_paragraphs
            .iter()
            .any(|paragraphs| paragraphs.as_ref().is_some_and(|paragraphs| !paragraphs.is_empty()));
        if has_structured {
            let pages: Vec<Vec<crate::pdf::structure::types::PdfParagraph>> = all_page_paragraphs
                .into_iter()
                .map(|opt| opt.unwrap_or_default())
                .collect();
            #[cfg(feature = "layout-detection")]
            let pages = {
                let mut pages = pages;
                crate::pdf::structure::adapters::promote_anchored_ordered_list_sequences(&mut pages);
                pages
            };
            // Document-global heading/list heuristic: `ocr_doc_to_paragraphs` /
            // `ocr_doc_to_layout_paragraphs` (above) build `pages` one OCR page at a
            // time and never call it, which is why every non-layout OCR route has
            // emitted zero headings and zero list items regardless of output format.
            // `None` (Plain output, or pages ML layout already classified) falls
            // through to the pre-existing assembly unchanged. Skipped entirely when
            // `skip_document_global_heuristic` is set -- see this function's own doc
            // comment: the caller wants the *bare* `pages` back to run this pass
            // itself, document-wide, and a heuristic run here first would either be
            // wasted (too few pages to cluster) or would mark `pages` "already
            // structured" and block that wider pass from doing anything.
            let doc = if skip_document_global_heuristic {
                Some(crate::pdf::structure::assemble_internal_document(
                    pages.clone(),
                    &collected_tables,
                    None,
                    &[],
                ))
            } else {
                match heuristically_restructured_ocr_pages(&pages, &collected_tables, config) {
                    Some(doc) => Some(doc),
                    None => {
                        // The document-global heading/list heuristic declined to run (Plain
                        // output, or a paragraph was already ML-layout-classified) or found
                        // nothing. Apply a text-marker-only list pass to the bare pages before
                        // falling back to plain assembly, so a non-layout OCR document that
                        // never reaches the heuristic still gets *some* list-item recovery
                        // (#713) -- this never touches `heading_level`, so it cannot regress
                        // heading detection the way pre-classifying before the heuristic call
                        // would (see `apply_ocr_text_list_fallback`'s own doc comment on the
                        // "already structured" gate this must not trip).
                        let mut fallback_pages = pages.clone();
                        for (page_idx, page) in fallback_pages.iter_mut().enumerate() {
                            apply_ocr_text_list_fallback(page);
                            // #729: recover a bare marker no ML hint ever classified --
                            // see `adapters::reattach_detached_ocr_list_markers`'s doc
                            // comment.
                            // #760: same rotation lookup as `assemble_ocr_page_paragraphs`'s
                            // call site above -- an explicit override wins, otherwise this
                            // page's own known PDF `/Rotate`, defaulting to `0`.
                            #[cfg(feature = "pdf")]
                            let page_rotation_degrees = if page_rotation_override != 0 {
                                page_rotation_override
                            } else {
                                lazy_pdf_render_state
                                    .as_ref()
                                    .and_then(|(_, _, rotations)| rotations.get(page_idx))
                                    .or_else(|| external_image_page_rotations.as_ref().and_then(|r| r.get(page_idx)))
                                    .copied()
                                    .unwrap_or(0)
                            };
                            #[cfg(not(feature = "pdf"))]
                            let page_rotation_degrees: u32 = 0;
                            crate::pdf::structure::adapters::reattach_detached_ocr_list_markers(
                                page,
                                page_rotation_degrees,
                            );
                        }
                        Some(crate::pdf::structure::assemble_internal_document(
                            fallback_pages,
                            &collected_tables,
                            None,
                            &[],
                        ))
                    }
                }
            };
            (doc, pages)
        } else {
            (None, Vec::new())
        }
    };

    #[cfg(feature = "pdf")]
    let ocr_doc = {
        let mut warnings = image_fallback_warnings;
        warnings.extend(recognition_noise_warnings);
        warnings.extend(page_failure_warnings);
        attach_ocr_fallback_warnings(ocr_doc, &result, warnings)
    };
    // Without `pdf` there is no page renderer, so no page-level OCR runs and the vector is
    // always empty; bind it so the non-pdf build does not warn about an unused value.
    #[cfg(not(feature = "pdf"))]
    let _ = (recognition_noise_warnings, page_failure_warnings);

    Ok((
        result,
        mean_text_conf,
        collected_tables,
        all_ocr_elements,
        ocr_doc,
        accumulated_llm_usage,
        page_texts,
        if capture_rasters { Some(captured_rasters) } else { None },
        accumulated_formulas,
        raw_page_paragraphs,
    ))
}

/// Build an [`crate::types::ExtractedImage`] for a full-page OCR raster.
///
/// `image_index` is set to 0; the caller must reindex after merging into
/// the document's image collection.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn build_page_raster_image(
    page_idx: usize,
    png_bytes: bytes::Bytes,
    width: u32,
    height: u32,
) -> crate::types::ExtractedImage {
    crate::types::ExtractedImage {
        data: png_bytes,
        format: std::borrow::Cow::Borrowed("png"),
        image_index: 0,
        page_number: Some((page_idx + 1) as u32),
        width: Some(width),
        height: Some(height),
        colorspace: Some("RGB".to_string()),
        bits_per_component: Some(8),
        is_mask: false,
        description: None,
        ocr_result: None,
        bounding_box: None,
        source_path: None,
        image_kind: Some(crate::types::ImageKind::PageRaster),
        kind_confidence: None,
        cluster_id: None,
        caption: None,
        qr_codes: None,
        data_base64: None,
    }
}

/// Adapt batch size to available system memory.
///
/// Estimates per-page memory cost based on typical page dimensions at 300 DPI
/// and compares against available system memory. Returns a batch size that
/// should keep peak memory within safe bounds.
///
/// Conservative estimate: each page in a batch needs approximately:
/// - ~50MB for render + encode working set (RGB buffer briefly, then PNG)
/// - ~100MB for OCR working set per concurrent page
/// - Plus the document itself and base allocations
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn adapt_batch_size_to_memory(configured: usize, document_size: usize) -> usize {
    let available_bytes = get_available_memory();

    if available_bytes == 0 {
        return configured;
    }

    let reserved = document_size + 512 * 1024 * 1024;
    let usable = available_bytes.saturating_sub(reserved);

    const PER_PAGE_ESTIMATE: usize = 150 * 1024 * 1024;

    let memory_limited_batch = (usable / PER_PAGE_ESTIMATE).max(1);

    let result = configured.min(memory_limited_batch);

    tracing::debug!(
        available_mb = available_bytes / (1024 * 1024),
        usable_mb = usable / (1024 * 1024),
        document_mb = document_size / (1024 * 1024),
        memory_limited_batch,
        configured,
        result,
        "OCR batch size adaptation"
    );

    result
}

/// Query available system memory without external dependencies.
///
/// On Linux (including Docker), reads `/proc/meminfo` for `MemAvailable`.
/// On macOS, uses `sysctl hw.memsize` for total memory (conservative fallback).
/// Returns 0 if the query fails, signaling the caller to use the default batch size.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn get_available_memory() -> usize {
    #[cfg(target_os = "linux")]
    {
        let host = read_meminfo_available();
        host.min(cgroup_headroom().unwrap_or(usize::MAX))
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("sysctl").args(["-n", "hw.memsize"]).output()
            && let Ok(s) = std::str::from_utf8(&output.stdout)
            && let Ok(total) = s.trim().parse::<usize>()
        {
            return total / 2;
        }
        0
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
fn parse_meminfo_available(contents: &str) -> usize {
    contents
        .lines()
        .find_map(|l| {
            l.strip_prefix("MemAvailable:")?
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<usize>()
                .ok()
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
fn read_meminfo_available() -> usize {
    parse_meminfo_available(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
fn parse_cgroup_v2(max: &str, current: &str) -> Option<usize> {
    let max = max.trim();
    if max == "max" {
        return None;
    }
    let limit = max.parse::<usize>().ok()?;
    let usage = current.trim().parse::<usize>().ok()?;
    Some(limit.saturating_sub(usage))
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
fn parse_cgroup_v1(limit: &str, usage: &str) -> Option<usize> {
    let limit = limit.trim().parse::<usize>().ok()?;
    let usage = usage.trim().parse::<usize>().ok()?;
    (limit < (isize::MAX as usize)).then(|| limit.saturating_sub(usage))
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), target_os = "linux"))]
fn cgroup_headroom() -> Option<usize> {
    if let (Ok(max), Ok(cur)) = (
        std::fs::read_to_string("/sys/fs/cgroup/memory.max"),
        std::fs::read_to_string("/sys/fs/cgroup/memory.current"),
    ) {
        return parse_cgroup_v2(&max, &cur);
    }
    let limit = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes").ok()?;
    let usage = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes").ok()?;
    parse_cgroup_v1(&limit, &usage)
}
/// Decide whether a pipeline stage's result should replace the current best-effort
/// candidate, given the pipeline's [`OcrPipelineSelection`](crate::core::config::OcrPipelineSelection) policy.
///
/// Only called once no stage has cleared `quality_thresholds.pipeline_min_quality` (the
/// accept-threshold early return in [`run_ocr_pipeline`] handles that case directly).
/// Pure and backend-free so the policy can be unit-tested without a registered OCR
/// backend.
///
/// - [`OcrPipelineSelection::HighestScore`]: replace only if `candidate_score` strictly
///   exceeds the current best score (or there is no current best). This is the original,
///   correctness-blind quality-max behavior.
/// - [`OcrPipelineSelection::PreferLastNonEmpty`]: replace whenever `candidate_text` is
///   non-empty, regardless of score, since a later stage in a fallback pipeline only ran
///   because the earlier stage(s) were judged inadequate. An empty candidate never
///   replaces an existing best, so a destroyed page still keeps the earlier text.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn should_replace_best_effort_result(
    selection: crate::core::config::OcrPipelineSelection,
    best_score: Option<f64>,
    candidate_text: &str,
    candidate_score: f64,
) -> bool {
    use crate::core::config::OcrPipelineSelection;

    match selection {
        OcrPipelineSelection::HighestScore => match best_score {
            Some(best) => candidate_score > best,
            None => true,
        },
        OcrPipelineSelection::PreferLastNonEmpty => !candidate_text.trim().is_empty() || best_score.is_none(),
    }
}

/// Attach skipped and failed stage diagnostics to the result that survives the pipeline.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn attach_ocr_pipeline_stage_warnings(
    mut doc: Option<crate::types::internal::InternalDocument>,
    text: &str,
    unavailable_backends: &[String],
    stage_failures: &[(String, String)],
) -> Option<crate::types::internal::InternalDocument> {
    if unavailable_backends.is_empty() && stage_failures.is_empty() {
        return doc;
    }

    let retained_doc = doc.get_or_insert_with(|| {
        let mut doc = crate::types::internal::InternalDocument::new("pdf");
        // Backend text verbatim (see `flat_ocr_page_document`): normalize before splitting.
        let text = crate::extraction::transform::normalize_line_endings(text);
        for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
            doc.push_element(crate::types::internal::InternalElement::text(
                crate::types::internal::ElementKind::Paragraph,
                paragraph,
                0,
            ));
        }
        doc
    });

    for backend in unavailable_backends {
        retained_doc.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("ocr_pipeline"),
            message: std::borrow::Cow::Owned(format!(
                "Requested OCR pipeline backend '{backend}' is unavailable and was skipped."
            )),
        });
    }
    for (backend, error) in stage_failures {
        retained_doc.processing_warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("ocr_pipeline"),
            message: std::borrow::Cow::Owned(format!(
                "OCR fallback backend '{backend}' failed and was skipped: {error}"
            )),
        });
    }

    doc
}

/// Attach force_ocr image-XObject fallback warnings (#1355) to the OCR-produced
/// document, mirroring [`attach_ocr_pipeline_stage_warnings`]'s `get_or_insert_with`
/// shape so the warning always survives even when no structured document was built.
//
// `ocr-pipeline` (not just `ocr`): the caller is inside `extract_with_ocr`
// (`any(ocr, ocr-pipeline)`), and the `binstall` CLI profile enables `ocr-pipeline`
// via `liter-llm` without `ocr`. ~keep
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
fn attach_ocr_fallback_warnings(
    mut doc: Option<crate::types::internal::InternalDocument>,
    text: &str,
    warnings: Vec<crate::types::ProcessingWarning>,
) -> Option<crate::types::internal::InternalDocument> {
    if warnings.is_empty() {
        return doc;
    }

    let retained_doc = doc.get_or_insert_with(|| {
        let mut doc = crate::types::internal::InternalDocument::new("pdf");
        // Backend text verbatim (see `flat_ocr_page_document`): normalize before splitting.
        let text = crate::extraction::transform::normalize_line_endings(text);
        for paragraph in text.split("\n\n").map(str::trim).filter(|text| !text.is_empty()) {
            doc.push_element(crate::types::internal::InternalElement::text(
                crate::types::internal::ElementKind::Paragraph,
                paragraph,
                0,
            ));
        }
        doc
    });

    retained_doc.processing_warnings.extend(warnings);

    doc
}

/// Last-resort image-XObject recovery for the OCR *pipeline* route, used when every stage
/// failed outright (#1444).
///
/// The per-page fallback inside [`extract_with_ocr_for_page`] covers a stage that ran and
/// came back blank. It cannot cover a stage that never produced a result at all — a VLM that
/// errored on every page, an unreachable service — because [`run_ocr_pipeline_for_page`]
/// catches that `Err` and moves on, and once the last stage is exhausted the pipeline used to
/// error out having never tried the pages' embedded images.
///
/// Uses the highest-priority *available* stage's backend, which is the one the pipeline
/// itself would have preferred. Returns `None` when there is no PDF content, no usable
/// backend, or no page yielded any text — the caller must then still report the failure.
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
#[allow(clippy::type_complexity)]
async fn recover_pipeline_document_from_image_xobjects(
    content: Option<&[u8]>,
    config: &ExtractionConfig,
    ocr_config: &crate::core::config::OcrConfig,
    backend_name: &str,
) -> Option<(
    String,
    Vec<String>,
    Vec<crate::types::ExtractedImage>,
    Vec<crate::types::ProcessingWarning>,
)> {
    let content = content?;
    let backend = {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let registry = registry.read();
        registry.get(backend_name).ok()?
    };
    let (doc, page_count, _) = open_pdf_for_full_ocr(content).ok()?;

    let mut page_texts = vec![String::new(); page_count];
    let mut recovered_images: Vec<crate::types::ExtractedImage> = Vec::new();
    let mut warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
    for (page_idx, page_text) in page_texts.iter_mut().enumerate() {
        let Some(recovery) = recover_page_text_from_image_xobjects(&backend, &doc, page_idx, ocr_config).await else {
            continue;
        };
        if !recovery.text.is_empty() {
            *page_text = recovery.text;
            warnings.push(xobject_fallback_warning(page_idx, recovery.attempted));
        }
        recovered_images.extend(recovery.images);
    }

    if page_texts.iter().all(|text| text.trim().is_empty()) {
        return None;
    }

    // Same page joining rule as `extract_with_ocr_for_page`, so a recovered document reads
    // identically to a normally-OCR'd one.
    let page_marker_cfg = config.pages.as_ref().filter(|pages| pages.insert_page_markers);
    let mut text = String::new();
    for (page_idx, page_text) in page_texts.iter().enumerate() {
        if let Some(cfg) = page_marker_cfg {
            text.push_str(&cfg.marker_format.replace("{page_num}", &(page_idx + 1).to_string()));
        } else if page_idx > 0 {
            text.push_str("\n\n");
        }
        text.push_str(page_text);
    }

    Some((text, page_texts, recovered_images, warnings))
}

/// Run a multi-backend OCR pipeline with quality-based fallback.
///
/// Images and layout detections are computed once and shared across all stages.
/// Each stage produces OCR output that is scored; if the score meets the
/// pipeline's quality threshold, the result is accepted. Otherwise, the next
/// backend is tried. If no stage clears the threshold, `pipeline.selection`
/// decides which stage's result is returned as the best effort: the
/// highest-scoring one ([`OcrPipelineSelection::HighestScore`], the default, used
/// for explicit and classical auto-fallback pipelines), or the last stage that
/// produced non-empty text ([`OcrPipelineSelection::PreferLastNonEmpty`], used by
/// `vlm_fallback`-synthesised pipelines -- see
/// [`should_replace_best_effort_result`]).
///
/// Thin, signature-preserving wrapper over [`run_ocr_pipeline_for_page`] with no page
/// rotation (`0`), so every pre-existing caller keeps today's behavior -- each stage's own
/// [`extract_with_ocr`] call falls back to its content-based per-page auto-detection.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) async fn run_ocr_pipeline(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    pipeline: &crate::core::config::OcrPipelineConfig,
    path: Option<&std::path::Path>,
) -> crate::Result<(
    String,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
)> {
    let (text, tables, elements, doc, usage, page_texts, rasters, formulas, _raw_page_paragraphs) =
        Box::pin(run_ocr_pipeline_for_page(
            content,
            images,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            config,
            pipeline,
            path,
            0,
            false,
            None,
        ))
        .await?;
    Ok((text, tables, elements, doc, usage, page_texts, rasters, formulas))
}

/// Same as [`run_ocr_pipeline`], but `page_rotation_degrees` -- the page's known PDF
/// `/Rotate` value, `0` if unrotated or genuinely unknown to the caller -- is forwarded to
/// every stage's [`extract_with_ocr_for_page`] call as its `page_rotation_override`. Each
/// stage backend still resolves its own [`crate::plugins::PageOrientationHandling`], so a
/// `RequiresUpright` stage gets an upright raster via `upright_raster_for_backend` while a
/// `SelfCorrecting` or `RecognisesRotatedText` stage's raster is left untouched (or only gets
/// the `ocr_config_with_page_rotation_hint` block-order hint) -- see `extract_with_ocr_for_page`.
///
/// Needed by a caller that drives a single page's image through the pipeline detached from
/// the rest of its document (currently only `extract_mixed_ocr_native`'s per-page pipeline
/// route, see #651): that caller already knows the page's own `/Rotate` value, but the
/// stage's own content-based auto-detection cannot recover it from a lone, index-0 image.
///
/// `skip_document_global_heuristic` is forwarded verbatim to every stage's
/// [`extract_with_ocr_for_page`] call, and this function's own extra return element -- the
/// winning stage's bare, unclassified per-page paragraphs -- is that stage's own extra
/// element, so callers get back exactly the material a stage never got to structure itself.
/// `points_per_pixel_override` is likewise forwarded verbatim to every stage's
/// [`extract_with_ocr_for_page`] call. See that function's doc comments for why both exist.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[allow(clippy::too_many_arguments)]
async fn run_ocr_pipeline_for_page(
    content: Option<&[u8]>,
    images: Option<&[image::DynamicImage]>,
    #[cfg(feature = "layout-detection")] layout_detections: Option<&[crate::layout::DetectionResult]>,
    config: &ExtractionConfig,
    pipeline: &crate::core::config::OcrPipelineConfig,
    path: Option<&std::path::Path>,
    page_rotation_degrees: u32,
    skip_document_global_heuristic: bool,
    points_per_pixel_override: Option<f32>,
) -> crate::Result<(
    String,
    Vec<crate::types::Table>,
    Vec<crate::types::OcrElement>,
    Option<crate::types::internal::InternalDocument>,
    Vec<crate::types::LlmUsage>,
    Vec<String>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
    Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
)> {
    use crate::plugins::registry::get_ocr_backend_registry;

    let default_ocr_config = crate::core::config::OcrConfig::default();
    let ocr_config = config.ocr.as_ref().unwrap_or(&default_ocr_config);

    // Best-effort selection policy is derived from the config that produced this pipeline
    // (a `vlm_fallback`-synthesised pipeline prefers its last non-empty stage; explicit and
    // classical pipelines stay score-based) rather than carried on `OcrPipelineConfig`, so
    // the binding-facing config stays unchanged (#1341).
    let selection = ocr_config.pipeline_selection();

    let mut stages = pipeline.stages.clone();
    stages.sort_by_key(|b| std::cmp::Reverse(b.priority));

    let requested_backends: Vec<String> = stages.iter().map(|s| s.backend.clone()).collect();
    let (available_stages, unavailable_backends): (Vec<_>, Vec<_>) = {
        let registry = get_ocr_backend_registry();
        let registry = registry.read();
        stages
            .into_iter()
            .partition(|stage| registry.get(&stage.backend).is_ok())
    };
    let unavailable_backends = unavailable_backends
        .into_iter()
        .map(|stage| stage.backend)
        .collect::<Vec<_>>();

    if available_stages.is_empty() {
        return Err(crate::XbergError::Parsing {
            message: format!(
                "No available OCR backends for pipeline (requested: {})",
                requested_backends.join(", ")
            ),
            source: None,
        });
    }

    #[allow(clippy::type_complexity)]
    let mut best_result: Option<(
        String,
        f64,
        Vec<crate::types::Table>,
        Vec<crate::types::OcrElement>,
        Option<crate::types::internal::InternalDocument>,
        Vec<String>,
        Option<Vec<crate::types::ExtractedImage>>,
        Vec<crate::types::Formula>,
        Vec<Vec<crate::pdf::structure::types::PdfParagraph>>,
    )> = None;

    let mut accumulated_usage: Vec<crate::types::LlmUsage> = Vec::new();
    // Track stages that errored outright (e.g. a VLM fallback that failed
    // authentication) so the failure is surfaced to the caller instead of being
    // silently replaced by a lower-quality earlier result (issue #1339).
    let mut stage_failures: Vec<(String, String)> = Vec::new();

    for stage in &available_stages {
        let mut stage_ocr = ocr_config.clone();
        stage_ocr.backend = stage.backend.clone();
        if let Some(ref lang) = stage.language {
            stage_ocr.language = lang.clone();
        }
        if let Some(ref tc) = stage.tesseract_config {
            stage_ocr.tesseract_config = Some(tc.clone());
        }
        if let Some(ref pc) = stage.paddle_ocr_config {
            stage_ocr.paddle_ocr_config = Some(pc.clone());
        }
        stage_ocr.vlm_config = stage.vlm_config.clone();
        stage_ocr.backend_options = stage.backend_options.clone();

        let stage_config = ExtractionConfig {
            ocr: Some(stage_ocr),
            ..config.clone()
        };

        tracing::debug!(
            backend = %stage.backend,
            priority = stage.priority,
            "Pipeline: trying OCR backend"
        );

        let result = Box::pin(extract_with_ocr_for_page(
            content,
            images,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            &stage_config,
            path,
            page_rotation_degrees,
            skip_document_global_heuristic,
            points_per_pixel_override,
        ))
        .await;

        match result {
            Ok((
                text,
                mean_conf,
                stage_tables,
                stage_ocr_elements,
                stage_doc,
                stage_llm_usage,
                stage_page_texts,
                stage_rasters,
                stage_formulas,
                stage_raw_paragraphs,
            )) => {
                let text_score = compute_quality_score(&text, &pipeline.quality_thresholds);
                let score = pipeline_stage_score(text_score, mean_conf);

                tracing::debug!(
                    backend = %stage.backend,
                    score,
                    text_score,
                    mean_text_conf = ?mean_conf,
                    threshold = pipeline.quality_thresholds.pipeline_min_quality,
                    "Pipeline: backend produced result"
                );

                accumulated_usage.extend(stage_llm_usage);

                if score >= pipeline.quality_thresholds.pipeline_min_quality {
                    // ~keep Attach prior-stage diagnostics before this accepted-stage early
                    // return; otherwise successful fallback silently erases why it ran.
                    let stage_doc =
                        attach_ocr_pipeline_stage_warnings(stage_doc, &text, &unavailable_backends, &stage_failures);
                    return Ok((
                        text,
                        stage_tables,
                        stage_ocr_elements,
                        stage_doc,
                        accumulated_usage,
                        stage_page_texts,
                        stage_rasters,
                        stage_formulas,
                        stage_raw_paragraphs,
                    ));
                }

                // Selection policy decides which stage's result to keep once no stage has
                // cleared the accept threshold (see `should_replace_best_effort_result`).
                // `HighestScore` (explicit / classical auto-fallback pipelines) keeps the
                // original strict quality-max behavior. `PreferLastNonEmpty`
                // (`vlm_fallback`-synthesised pipelines) prefers the deepest non-empty
                // fallback instead: stages run in priority order (primary first), so a
                // later non-empty result was invoked precisely because the higher-priority
                // stages were inadequate, and a correctness-blind score-max heuristic can
                // otherwise pin selection to an inadequate primary (e.g. merged-word
                // tesseract text scoring above a correct VLM transcription), discarding the
                // very fallback the pipeline ran (#1341). An empty fallback never
                // overwrites, so the earlier text is still kept in that case.
                let best_score = best_result.as_ref().map(|(_, best_score, ..)| *best_score);
                if should_replace_best_effort_result(selection, best_score, &text, score) {
                    best_result = Some((
                        text,
                        score,
                        stage_tables,
                        stage_ocr_elements,
                        stage_doc,
                        stage_page_texts,
                        stage_rasters,
                        stage_formulas,
                        stage_raw_paragraphs,
                    ));
                }
            }
            Err(e) => {
                tracing::warn!(
                    backend = %stage.backend,
                    error = %e,
                    "Pipeline: backend failed, trying next"
                );
                stage_failures.push((stage.backend.clone(), e.to_string()));
            }
        }
    }

    match best_result {
        Some((text, score, tables, elements, doc, page_texts, rasters, formulas, raw_page_paragraphs)) => {
            let threshold = pipeline.quality_thresholds.pipeline_min_quality;
            tracing::warn!(
                score,
                threshold,
                selection = ?selection,
                "All OCR pipeline backends produced suboptimal quality, using best-effort result \
                 selected per the pipeline's selection policy"
            );
            let mut doc = doc.unwrap_or_else(|| {
                let mut d = crate::types::internal::InternalDocument::new("pdf");
                // Backend text verbatim (see `flat_ocr_page_document`). This best-effort arm
                // is where `PreferLastNonEmpty` lands VLM output, the most likely CR source.
                let text = crate::extraction::transform::normalize_line_endings(&text);
                for paragraph in text.split("\n\n") {
                    let trimmed = paragraph.trim();
                    if !trimmed.is_empty() {
                        d.push_element(crate::types::internal::InternalElement::text(
                            crate::types::internal::ElementKind::Paragraph,
                            trimmed,
                            0,
                        ));
                    }
                }
                d
            });
            doc.processing_warnings.push(crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("ocr_pipeline"),
                message: std::borrow::Cow::Owned(format!(
                    "All OCR pipeline backends scored below the configured quality threshold \
                     (best score {score:.3} < {threshold:.3}); returning the best-effort result \
                     chosen by the pipeline's {:?} selection policy, which may be inaccurate or \
                     incomplete.",
                    selection
                )),
            });
            let doc = attach_ocr_pipeline_stage_warnings(Some(doc), &text, &unavailable_backends, &stage_failures);
            Ok((
                text,
                tables,
                elements,
                doc,
                accumulated_usage,
                page_texts,
                rasters,
                formulas,
                raw_page_paragraphs,
            ))
        }
        None => {
            // #1444: every stage errored, so no stage ever reached its own per-page
            // image-XObject fallback. Try the pages' embedded images once here before
            // giving up -- for a scanned PDF whose rasterizer output is blank, those
            // images are the only place the text ever was.
            #[cfg(feature = "pdf")]
            if let Some(first_stage) = available_stages.first()
                && let Some((text, page_texts, recovered_images, warnings)) = Box::pin(
                    recover_pipeline_document_from_image_xobjects(content, config, ocr_config, &first_stage.backend),
                )
                .await
            {
                let doc = attach_ocr_pipeline_stage_warnings(None, &text, &unavailable_backends, &stage_failures);
                let doc = attach_ocr_fallback_warnings(doc, &text, warnings);
                let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
                return Ok((
                    text,
                    Vec::new(),
                    Vec::new(),
                    doc,
                    accumulated_usage,
                    page_texts,
                    if capture_rasters { Some(recovered_images) } else { None },
                    Vec::new(),
                    Vec::new(),
                ));
            }

            let detail = if stage_failures.is_empty() {
                String::new()
            } else {
                let causes = stage_failures
                    .iter()
                    .map(|(backend, error)| format!("{backend}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(" ({causes})")
            };
            Err(crate::XbergError::Parsing {
                message: format!("All OCR pipeline backends failed{detail}"),
                source: None,
            })
        }
    }
}

/// Clone an OCR config with word-level elements forced on for structure consumers.
///
/// Table recognition requires word geometry while semantic paragraph assembly
/// consumes the backend's line-only internal document even without ML layout.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ensure_elements_enabled(config: &crate::core::config::ocr::OcrConfig) -> crate::core::config::ocr::OcrConfig {
    let mut config = config.clone();
    match config.element_config.as_mut() {
        Some(ec) => {
            ec.include_elements = true;
            ec.min_level = crate::types::OcrElementLevel::Word;
        }
        None => {
            config.element_config = Some(crate::types::OcrElementConfig {
                include_elements: true,
                min_level: crate::types::OcrElementLevel::Word,
                ..Default::default()
            });
        }
    }
    config
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn filter_public_ocr_elements(
    elements: &[crate::types::OcrElement],
    config: &crate::core::config::ocr::OcrConfig,
) -> Vec<crate::types::OcrElement> {
    let Some(element_config) = config.element_config.as_ref().filter(|config| config.include_elements) else {
        return Vec::new();
    };

    let minimum_rank = ocr_element_level_rank(element_config.min_level);

    elements
        .iter()
        .filter(|element| element.confidence.recognition >= element_config.min_confidence)
        .filter(|element| ocr_element_level_rank(element.level) >= minimum_rank)
        .cloned()
        .collect()
}

/// Inject a page's PDF `/Rotate` value into `OcrConfig.backend_options` so a
/// backend that can use it (currently PaddleOCR, see
/// `paddle_ocr::backend::PaddleOcrBackend::reorder_blocks_for_page_rotation`) can
/// correct its detector's raster-space block order into true reading order
/// (#640). `normalize_rendered_page_for_ocr` deliberately hands every backend a
/// raster in the page's raw MediaBox orientation rather than its display
/// orientation (see the #530 regression test in `crate::pdf::render`), which
/// Tesseract's own layout analysis already reads correctly; PaddleOCR's detector
/// does not, so this hint is the minimal, backend-local fix rather than changing
/// the shared raster every backend (including Tesseract) receives.
///
/// A no-op for `page_rotation_degrees == 0` (the overwhelmingly common case) so
/// unrotated pages never pay a config clone. Backends that don't recognise the
/// `page_rotation_degrees` key ignore it, per `OcrConfig.backend_options`'s
/// documented contract.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn ocr_config_with_page_rotation_hint(
    config: &crate::core::config::ocr::OcrConfig,
    page_rotation_degrees: u32,
) -> Cow<'_, crate::core::config::ocr::OcrConfig> {
    if page_rotation_degrees == 0 {
        return Cow::Borrowed(config);
    }
    let mut config = config.clone();
    let mut opts = config.backend_options.take().unwrap_or_else(|| serde_json::json!({}));
    if !opts.is_object() {
        opts = serde_json::json!({});
    }
    if let Some(obj) = opts.as_object_mut() {
        obj.insert(
            "page_rotation_degrees".to_string(),
            serde_json::Value::Number(page_rotation_degrees.into()),
        );
    }
    config.backend_options = Some(opts);
    Cow::Owned(config)
}

/// Rotate a page raster upright before handing it to a backend that cannot cope with a
/// sideways page (`PageOrientationHandling::RequiresUpright`), returning the bytes/dimensions
/// actually sent to the backend and the rotation (in `image`-crate terms: one of `0`, `90`,
/// `180`, `270`) that must later be undone on any pixel-space geometry the backend returns.
///
/// `normalize_rendered_page_for_ocr` deliberately hands every backend a raster in the page's
/// raw MediaBox orientation rather than its display orientation (#530), which Tesseract
/// (`SelfCorrecting`) reads fine and PaddleOCR (`RecognisesRotatedText`) recognises correctly
/// (only its block order needs fixing, see `ocr_config_with_page_rotation_hint`). A backend
/// that declares `RequiresUpright` (currently sceptre) produces character garbage on that same
/// sideways raster instead — measured this session on `/Rotate 270` scanned pages, and
/// confirmed by feeding the same page through an upright render. This is the backend-local
/// fix: re-apply the page's original `/Rotate` value on top of the already-corrected raster,
/// undoing exactly what `normalize_rendered_page_for_ocr` undid, so only that backend's input
/// changes (#643).
///
/// A no-op — returns `data` unchanged and a correction of `0` — when `page_rotation_degrees ==
/// 0` or `orientation_handling` is not `RequiresUpright`. In particular, `SelfCorrecting` and
/// `RecognisesRotatedText` backends never pay a re-encode and never see a different raster than
/// they do today: Tesseract's input is untouched by this function under every input.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn upright_raster_for_backend(
    data: &std::sync::Arc<Vec<u8>>,
    width: u32,
    height: u32,
    page_rotation_degrees: u32,
    orientation_handling: crate::plugins::PageOrientationHandling,
) -> crate::Result<(std::sync::Arc<Vec<u8>>, u32, u32, u32)> {
    if page_rotation_degrees == 0 || orientation_handling != crate::plugins::PageOrientationHandling::RequiresUpright {
        return Ok((std::sync::Arc::clone(data), width, height, 0));
    }
    let correction_degrees = page_rotation_degrees % 360;
    let (rotated, new_width, new_height) =
        crate::pdf::render::rotate_png_page_if_needed((**data).clone(), width, height, correction_degrees)?;
    Ok((std::sync::Arc::new(rotated), new_width, new_height, correction_degrees))
}

/// Undo [`upright_raster_for_backend`]'s rotation on a backend result's pixel-space geometry,
/// mapping bboxes from the upright raster the backend actually ran OCR on back to the raw
/// MediaBox raster every downstream consumer (`build_mixed_ocr_page_document`,
/// `rescale_ocr_bboxes_to_page_points`, the #530 regression test) expects.
///
/// A no-op when `correction_degrees == 0` (the overwhelmingly common case: no correction was
/// applied at all). Reuses [`undo_auto_rotate_point`] — the same per-point inverse
/// [`undo_auto_rotate_document_bboxes`] uses for a backend's own internal auto-rotation — since
/// the geometry problem is identical: a point in a rotated raster mapped back to the unrotated
/// one it will be rescaled against.
///
/// Only `ocr_internal_document` element bboxes and `tables` bboxes are corrected: those are the
/// two pixel-space geometry sources `build_mixed_ocr_page_document` reads from a backend result
/// before rescaling into page points.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn undo_upright_raster_correction(
    result: &mut crate::types::ExtractedDocument,
    correction_degrees: u32,
    upright_width: u32,
    upright_height: u32,
) {
    if correction_degrees == 0 {
        return;
    }
    let correction_degrees = correction_degrees as u16;
    let (processed_width, processed_height) = (f64::from(upright_width), f64::from(upright_height));
    let undo_bbox = |bbox: &mut crate::types::extraction::BoundingBox| {
        let (x0, y0) = undo_auto_rotate_point(bbox.x0, bbox.y0, correction_degrees, processed_width, processed_height);
        let (x1, y1) = undo_auto_rotate_point(bbox.x1, bbox.y1, correction_degrees, processed_width, processed_height);
        bbox.x0 = x0.min(x1);
        bbox.x1 = x0.max(x1);
        bbox.y0 = y0.min(y1);
        bbox.y1 = y0.max(y1);
    };
    if let Some(doc) = result.ocr_internal_document.as_mut() {
        for element in &mut doc.elements {
            if let Some(bbox) = element.bbox.as_mut() {
                undo_bbox(bbox);
            }
        }
    }
    for table in &mut result.tables {
        if let Some(bbox) = table.bounding_box.as_mut() {
            undo_bbox(bbox);
        }
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn ocr_element_level_rank(level: crate::types::OcrElementLevel) -> u8 {
    match level {
        crate::types::OcrElementLevel::Word => 0,
        crate::types::OcrElementLevel::Line => 1,
        crate::types::OcrElementLevel::Block => 2,
        crate::types::OcrElementLevel::Page => 3,
    }
}

/// Inject layout-detection settings into OcrConfig backend options for paired-mode backends.
///
/// When layout detection is active and provides detections, certain backends (e.g., GLM-OCR)
/// may need configuration injected from the layout-detection config. This function ensures
/// that the `enable_chart_understanding` flag from `ExtractionConfig.layout` is propagated
/// to the OCR backend via `backend_options` so per-region task dispatch can honor it.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn inject_layout_config_to_backend(
    config: &crate::core::config::ocr::OcrConfig,
    extraction_config: &ExtractionConfig,
) -> crate::core::config::ocr::OcrConfig {
    let mut config = config.clone();
    if let Some(layout_cfg) = &extraction_config.layout {
        let mut opts = config.backend_options.take().unwrap_or_else(|| serde_json::json!({}));

        if !opts.is_object() {
            if !opts.is_null() {
                tracing::warn!(
                    backend_options = %opts,
                    "backend_options was not a JSON object; replacing with new object to inject enable_chart_understanding"
                );
            }
            opts = serde_json::json!({});
        }

        if let Some(obj) = opts.as_object_mut() {
            obj.insert(
                "enable_chart_understanding".to_string(),
                serde_json::Value::Bool(layout_cfg.enable_chart_understanding),
            );
        }

        config.backend_options = Some(opts);
    }
    config
}

#[cfg(all(test, feature = "ocr"))]
mod tests {
    use super::*;

    #[cfg(feature = "ocr")]
    fn t() -> OcrQualityThresholds {
        OcrQualityThresholds::default()
    }

    #[cfg(feature = "ocr")]
    fn boundary(page_number: u32, byte_start: usize, byte_end: usize) -> crate::types::PageBoundary {
        crate::types::PageBoundary {
            byte_start,
            byte_end,
            page_number,
        }
    }

    /// A page whose OCR came back effectively blank must NOT overwrite that page's native
    /// text. Reported against a 16-page scanned ordinance whose page renders were empty:
    /// tesseract returned a couple of noise characters per page, every one cleared the old
    /// `!text.trim().is_empty()` bar, and the accepted replacements deleted the native text
    /// -- so enabling OCR returned FEWER characters than not enabling it (174 vs 185).
    #[test]
    #[cfg(feature = "ocr")]
    fn should_reject_blank_ocr_page_rather_than_overwrite_native_text() {
        let native = "Page one has real native content here.\nPage two also has real content.";
        let split = native.find('\n').expect("newline") + 1;
        let boundaries = vec![boundary(1, 0, split), boundary(2, split, native.len())];

        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        // What a blank page render actually produces: a scrap of noise.
        ocr_results.insert(1, "  a \n".to_string());

        let accepted = accepted_ocr_page_replacements(native, &boundaries, &ocr_results);
        assert!(
            accepted.is_empty(),
            "blank OCR output must not be accepted as a replacement, got {accepted:?}"
        );

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        assert_eq!(merged, native, "native text must survive a blank OCR page untouched");
        assert!(
            merged.chars().filter(|c| !c.is_whitespace()).count()
                >= native.chars().filter(|c| !c.is_whitespace()).count(),
            "OCR must never reduce the non-whitespace character count below native"
        );
    }

    /// The complement: real OCR text still replaces the page, so the guard above cannot be
    /// satisfied by simply refusing everything.
    #[test]
    #[cfg(feature = "ocr")]
    fn should_still_accept_ocr_page_with_real_recovered_text() {
        let native = "garbled\nPage two native.";
        let split = native.find('\n').expect("newline") + 1;
        let boundaries = vec![boundary(1, 0, split), boundary(2, split, native.len())];

        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(1, "ORDINANCE NO. 2197\n".to_string());

        let accepted = accepted_ocr_page_replacements(native, &boundaries, &ocr_results);
        assert_eq!(accepted.len(), 1, "a page with real OCR text must be accepted");

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        assert!(merged.contains("ORDINANCE NO. 2197"), "OCR text must reach the output");
        assert!(merged.contains("Page two native."), "untouched pages must be preserved");
    }

    /// Replacing a page with OCR text of a different length shifts every later offset, so
    /// the native boundaries no longer describe the merged text. Anything mapping a byte
    /// offset back to a page -- including page tagging, which is what `doc.pages` is built
    /// from -- reads the wrong page without this re-map.
    #[test]
    #[cfg(feature = "ocr")]
    fn should_remap_page_boundaries_onto_the_merged_text() {
        let native = "AAA\nBBB\nCCC";
        let boundaries = vec![boundary(1, 0, 4), boundary(2, 4, 8), boundary(3, 8, native.len())];

        let mut accepted: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        accepted.insert(2, "LONGER PAGE TWO\n".to_string());

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        let remapped = boundaries_after_replacements(&boundaries, &accepted);

        assert_eq!(remapped.len(), 3);
        // Page 1 is before the replacement: unchanged.
        assert_eq!((remapped[0].byte_start, remapped[0].byte_end), (0, 4));
        // Page 2 grew to the replacement's length.
        assert_eq!(remapped[1].byte_start, 4);
        assert_eq!(remapped[1].byte_end, 4 + "LONGER PAGE TWO\n".len());
        // Page 3 shifted by the delta, and must still address its own text.
        assert_eq!(remapped[2].byte_end, merged.len());
        assert_eq!(
            &merged[remapped[2].byte_start..remapped[2].byte_end],
            "CCC",
            "page 3 must still resolve to its own content after the shift"
        );
        assert_eq!(
            &merged[remapped[1].byte_start..remapped[1].byte_end],
            "LONGER PAGE TWO\n",
            "page 2 must resolve to the OCR text that replaced it"
        );
    }

    /// Issue #181: TATR tables recognized during full-document OCR must carry a
    /// deterministic `table_id`, `columns`, and `bounding_box` derived from
    /// `detection_bbox` — not `..Default::default()` blanks.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn recognized_table_to_public_table_assigns_id_columns_and_bounding_box() {
        let recognized = crate::RecognizedTable {
            detection_bbox: crate::layout::BBox::new(10.0, 20.0, 110.0, 220.0),
            cells: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            markdown: "| Name | Age |\n|---|---|\n| Alice | 30 |".to_string(),
        };

        let table = recognized_table_to_public_table(&recognized, 3, 1);

        assert_eq!(table.page_number, 3);
        assert_eq!(table.table_id.as_deref(), Some("table-2"));
        assert_eq!(table.columns, Some(vec!["Name".to_string(), "Age".to_string()]));
        let bbox = table.bounding_box.expect("bounding box must be populated");
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 110.0);
        assert_eq!(bbox.y1, 220.0);
    }

    /// Minimal `PdfParagraph` fixture: same shape `ocr_doc_to_layout_paragraphs`
    /// produces (empty `lines`, no layout region/caption), just enough for
    /// `apply_ocr_text_list_fallback`. Not layout-gated: `apply_ocr_text_list_fallback`
    /// itself no longer requires `layout-detection` (#713), and this fixture is reused
    /// by the non-layout-route tests below.
    #[cfg(feature = "ocr")]
    fn ocr_paragraph(text: &str) -> crate::pdf::structure::types::PdfParagraph {
        crate::pdf::structure::types::PdfParagraph {
            text: text.to_string(),
            lines: Vec::new(),
            dominant_font_size: 12.0,
            heading_level: None,
            is_bold: false,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count: text.split_whitespace().count(),
        }
    }

    /// Issue #695: on the OCR + `--layout` route, `ocr_doc_to_layout_paragraphs`
    /// only sets `is_list_item` from a high-confidence `ListItem` layout hint and
    /// never from the paragraph's own text, unlike the native-PDF assembler's
    /// `looks_like_list_item`. When the layout model fails to emit (or mislabels)
    /// a `ListItem` box for a numbered/bulleted line, the paragraph is built with
    /// `is_list_item: false` and stays that way -- exactly what a plain
    /// `ocr_doc_to_layout_paragraphs` call (without the fallback this test
    /// exercises) would leave behind.
    ///
    /// Without `apply_ocr_text_list_fallback`, every assertion below except the
    /// first (heading is left alone, which was already true before this change)
    /// fails: a fresh paragraph carries `is_list_item: false` by construction, and
    /// nothing in this call path would ever flip it.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_classifies_unclassified_numbered_and_bulleted_paragraphs() {
        let mut paragraphs = vec![
            ocr_paragraph("1. As shown on Exhibit B-1, the PD encompasses 0.7906 acre."),
            ocr_paragraph("2. Land Use: Live/Work Townhomes"),
            ocr_paragraph("- a dash-bulleted item"),
            ocr_paragraph("• a bullet-marked item"),
        ];

        apply_ocr_text_list_fallback(&mut paragraphs);

        assert!(paragraphs[0].is_list_item, "numbered marker '1.' must be recovered");
        assert!(paragraphs[1].is_list_item, "numbered marker '2.' must be recovered");
        assert!(paragraphs[2].is_list_item, "dash marker '- ' must be recovered");
        assert!(paragraphs[3].is_list_item, "bullet marker must be recovered");
    }

    /// A GENUINE numbered-section heading is left untouched even though it shares
    /// the "digit, separator, text" shape a list marker has: "1. INTRODUCTION" is
    /// excluded by `looks_like_list_item` itself (via `is_numbered_section_heading`,
    /// its ALL-CAPS-remainder branch), so this paragraph never reaches the override
    /// branch at all. This assertion passes with or without the fix -- it pins the
    /// "a real heading is never reclassified" half of the contract that the next
    /// two tests exercise from the other direction (a heading whose text is NOT a
    /// genuine section heading).
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_never_overrides_a_genuine_numbered_section_heading() {
        let mut paragraph = ocr_paragraph("1. INTRODUCTION");
        paragraph.heading_level = Some(2);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert_eq!(
            paragraph.heading_level,
            Some(2),
            "an existing heading must survive untouched"
        );
        assert!(
            !paragraph.is_list_item,
            "a paragraph already classified as a heading must not also become a list item"
        );
    }

    /// Regression for the `--ocr-scanned-pages --layout` list-F1 collapse (task
    /// #665): on `ordinance_2197_scanned.pdf` the layout model classified
    /// `"8. Maximum height of structures: 50'"` as a `Title` heading rather than a
    /// `ListItem`. Its text is NOT a genuine numbered section heading -- unlike
    /// "1. INTRODUCTION" above, the remainder is mixed-case prose, so
    /// `is_numbered_section_heading` (and therefore the exclusion inside
    /// `looks_like_list_item`) does not fire -- so the text-marker signal must win.
    ///
    /// Against the pre-fix code (which `continue`s whenever `heading_level.is_some()`,
    /// unconditionally), both assertions fail: `is_list_item` stays `false` and
    /// `heading_level` stays `Some(2)`.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_overrides_a_misclassified_title_heading() {
        let mut paragraph = ocr_paragraph("8. Maximum height of structures: 50'");
        paragraph.heading_level = Some(2);
        paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::Title);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an unambiguous list marker must win over a Title heading hint"
        );
        assert_eq!(
            paragraph.heading_level, None,
            "heading_level must be cleared: assembly.rs checks heading_level BEFORE is_list_item, \
             so leaving it set would silently render this as a heading regardless of is_list_item"
        );
    }

    /// Same override, exercised through a `SectionHeader` hint instead of `Title`
    /// (the two layout classes `should_promote_logo_followed_by_title` /
    /// `apply_hint_to_paragraph` can assign `heading_level` from), and at a
    /// different heading level, to show the override is not level- or
    /// hint-class-specific.
    ///
    /// Against the pre-fix code, both assertions fail for the same reason as above.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_overrides_a_misclassified_section_header_heading() {
        let mut paragraph = ocr_paragraph("(2) second item continues the numbered run");
        paragraph.heading_level = Some(3);
        paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::SectionHeader);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an unambiguous list marker must win over a SectionHeader heading hint"
        );
        assert_eq!(
            paragraph.heading_level, None,
            "heading_level must be cleared alongside setting is_list_item"
        );
    }

    /// A paragraph already marked as a list item by a high-confidence layout hint
    /// must be left exactly as-is: the fallback only fills gaps, it never
    /// re-derives a classification the layout path already made.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_leaves_already_classified_list_items_untouched() {
        let mut paragraph = ocr_paragraph("plain prose with no marker at all");
        paragraph.is_list_item = true;

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an existing list classification must survive untouched"
        );
    }

    /// Plain prose -- the overwhelming majority of any document's paragraphs --
    /// must never be swept into the list classification. Without this guard the
    /// fallback would be a net regression, not a fix.
    ///
    /// Asserted through `apply_ocr_text_list_fallback` rather than the marker
    /// predicate directly: the predicate is now
    /// `pdf::structure::pipeline::looks_like_list_item`, which owns its own tests,
    /// and what matters here is that the OCR route consults it and acts on the
    /// answer. The author-byline case ("A. Smith, B. Jones") is included because
    /// it is exactly what an OCR-route-local re-implementation of the marker rules
    /// would get wrong -- it only passes because the shared predicate is used.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_leaves_ordinary_prose_and_non_list_shapes_unclassified() {
        let texts = [
            "This is an ordinary sentence with no marker.",
            "1000. Four-digit identifiers are not list markers.",
            "2024. A total of 3 trucks were used.",
            "1. INTRODUCTION",
            "A. Smith, B. Jones, Journal of Irreproducible Results",
            "",
        ];
        let mut paragraphs: Vec<_> = texts.iter().map(|text| ocr_paragraph(text)).collect();

        apply_ocr_text_list_fallback(&mut paragraphs);

        for (paragraph, text) in paragraphs.iter().zip(texts) {
            assert!(!paragraph.is_list_item, "must not classify as a list item: {text:?}");
        }
    }

    /// #713: on the non-layout OCR route, `apply_ocr_text_list_fallback` was gated on
    /// `feature = "layout-detection"` even though it does no layout classification at
    /// all -- it is a pure text-marker pass. This is deliberately compiled and run
    /// under `feature = "ocr"` alone (no `layout-detection` in this test's own `cfg`)
    /// to prove it no longer requires that feature.
    ///
    /// Against unfixed code this does not compile in an `ocr`-only build (the function
    /// itself carries `#[cfg(all(feature = "ocr", feature = "layout-detection"))]`); in
    /// a build where `layout-detection` happens to be on too (the common CI default),
    /// it still documents the intended availability contract this fix changes.
    #[cfg(feature = "ocr")]
    #[test]
    fn apply_ocr_text_list_fallback_is_available_without_layout_detection() {
        let mut paragraphs = vec![ocr_paragraph("1. Item without any ML layout classification")];

        apply_ocr_text_list_fallback(&mut paragraphs);

        assert!(paragraphs[0].is_list_item);
    }

    /// #713 end-to-end for the non-layout OCR route: a single hOCR block spanning
    /// three numbered items must, after both fixes (`ocr_doc_to_paragraphs`'s
    /// marker-splitting and `apply_ocr_text_list_fallback`'s text-marker
    /// classification), come out as three separately classified list items -- the
    /// same outcome the ML-layout route already got via `push_body_group` +
    /// `apply_ocr_text_list_fallback`.
    ///
    /// Against unfixed code this asserts `paragraphs.len() == 3` and
    /// `paragraphs[i].is_list_item` for each; today `ocr_doc_to_paragraphs` returns a
    /// single unsplit, unclassified paragraph (`len() == 1`, `is_list_item == false`).
    #[cfg(feature = "ocr")]
    #[test]
    fn non_layout_route_recovers_list_items_from_a_multi_marker_ocr_block() {
        let mut doc = crate::types::internal::InternalDocument::new("test");
        let mut elem = crate::types::internal::InternalElement::text(
            crate::types::internal::ElementKind::OcrText {
                level: crate::types::ocr_elements::OcrElementLevel::Block,
            },
            "1. First item\n2. Second item\n3. Third item",
            0,
        );
        elem.bbox = Some(crate::types::extraction::BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 100.0,
        });
        doc.push_element(elem);

        let mut paragraphs = crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
            &doc,
            1000,
            crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
        );
        apply_ocr_text_list_fallback(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            3,
            "each marker-opening line must become its own paragraph"
        );
        assert!(
            paragraphs.iter().all(|paragraph| paragraph.is_list_item),
            "every split item must be classified"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_empty_text_triggers_fallback() {
        let decision = evaluate_native_text_for_ocr("", Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_replacement_chars_trigger_fallback() {
        let text = "The \u{FFFD}\u{FFFD}\u{FFFD} quick \u{FFFD}\u{FFFD}\u{FFFD} brown fox";
        let stats = NativeTextStats::from(text);
        assert_eq!(stats.garbage_char_count, 6);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_fragmented_words_trigger_fallback() {
        let text = "T h e q u i c k b r o w n f o x j u m p s";
        let stats = NativeTextStats::from(text);
        assert!(stats.fragmented_word_ratio > 0.8);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_good_text_no_fallback() {
        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences that form a coherent text block.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    /// Builds a PUA-heavy string simulating an undecodable glyph-index text layer:
    /// a font whose CID/glyph indices resolve into the Private Use Area rather than
    /// real Unicode (issue #1254).
    #[cfg(feature = "ocr")]
    fn pua_garbage_text() -> String {
        (0..200)
            .map(|i| char::from_u32(0xE000 + (i % 400)).expect("valid PUA codepoint"))
            .collect::<String>()
            .chars()
            .collect::<Vec<char>>()
            .chunks(6)
            .map(|chunk| chunk.iter().collect::<String>())
            .collect::<Vec<String>>()
            .join(" ")
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_flags_pua_heavy_text() {
        let garbage = pua_garbage_text();
        let stats = NativeTextStats::from(&garbage);
        assert!(
            stats.undecodable_ratio >= 0.99,
            "expected near-total undecodable ratio for all-PUA text, got {}",
            stats.undecodable_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_ignores_occasional_symbols() {
        let text = "This is a normal paragraph with meaningful words \u{2022} and one bullet symbol, \
                    plus a trademark\u{2122} and a section sign \u{00A7} sprinkled in for good measure.";
        let stats = NativeTextStats::from(text);
        assert!(
            stats.undecodable_ratio < 0.05,
            "expected a near-zero undecodable ratio for normal prose with a few symbols, got {}",
            stats.undecodable_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_excludes_cjk_kana_hangul_emoji() {
        let text = "\u{65E5}\u{672C}\u{8A9E} \u{D55C}\u{AD6D}\u{C5B4} \u{4E2D}\u{6587} \
                    \u{3072}\u{3089}\u{304C}\u{306A} \u{30AB}\u{30BF}\u{30AB}\u{30CA} \
                    with latin words and emoji \u{1F600}\u{1F680}";
        let stats = NativeTextStats::from(text);
        assert_eq!(
            stats.undecodable_ratio, 0.0,
            "CJK/Kana/Hangul/emoji must not count as undecodable, got {}",
            stats.undecodable_ratio
        );
    }

    /// A text layer that decodes almost entirely into the Unicode Private Use Area — the
    /// signature of a `Type0`/`Identity-H` font with `CIDToGIDMap /Identity`, no
    /// `/ToUnicode` CMap, and an embedded subset with neither `cmap` nor `post` — must be
    /// routed to OCR exactly like a scanned page, even though it has a full, visible,
    /// glyph-rich text layer (issue #1254).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_text_layer_routes_to_ocr() {
        let garbage = pua_garbage_text();
        let decision = evaluate_native_text_for_ocr(&garbage, Some(1), &t());
        assert!(
            decision.fallback,
            "a page whose text layer is mostly undecodable glyph indices must trigger OCR fallback"
        );
    }

    /// Normal prose that happens to contain a handful of real symbols (bullets, trademark
    /// signs, section marks) must NOT be misclassified as an undecodable text layer.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_with_symbols_does_not_route_to_ocr() {
        let text = "This is a normal paragraph with meaningful words \u{2022} and one bullet symbol, \
                    plus a trademark\u{2122} and a section sign \u{00A7} sprinkled in for good measure. \
                    It contains multiple sentences that form a coherent, legible text block.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(
            !decision.fallback,
            "normal prose with a few symbols must not trigger OCR fallback via the undecodable-ratio signal"
        );
    }

    /// Builds a gate decision with explicit fallback / whole-document-failure
    /// flags and otherwise-empty stats, for exercising `evaluate_ocr_skip_gate`
    /// independently of the native-text heuristics.
    #[cfg(feature = "ocr")]
    fn gate_decision(fallback: bool, whole_doc_failure: bool) -> OcrFallbackDecision {
        OcrFallbackDecision {
            stats: NativeTextStats::from(""),
            avg_non_whitespace: 0.0,
            avg_alnum: 0.0,
            fallback,
            failing_pages: Vec::new(),
            whole_doc_failure,
        }
    }

    /// A scanned page with a garbage/undecodable text layer produces a
    /// pre-rendered structured doc plus enough low-alphanumeric characters to
    /// look "non-textual", but the per-document check flags the whole document.
    /// The whole-document failure must win over the non-text skip and route to
    /// OCR, otherwise a scanner PDF is silently returned as empty native text
    /// (issue #1338).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_whole_doc_failure_overrides_non_text_skip() {
        let thresholds = t();
        let outcome = evaluate_ocr_skip_gate(
            true, // pre-rendered structured doc present
            50,
            0.1, // < alnum_ws_ratio_threshold (0.4): looks non-textual
            &gate_decision(true, true),
            &thresholds,
        );
        assert_eq!(
            outcome,
            OcrGateOutcome::RunFallback,
            "a whole-document quality failure must route to OCR, not SkipNonText"
        );
    }

    /// A genuinely non-textual *structured* document (a rendered diagram whose
    /// stray label characters are mostly punctuation) that still passes the
    /// per-document quality check must keep skipping OCR — the guard must not
    /// over-trigger and OCR every diagram.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_non_text_structured_doc_still_skips_ocr() {
        let thresholds = t();
        let outcome = evaluate_ocr_skip_gate(true, 50, 0.1, &gate_decision(false, false), &thresholds);
        assert_eq!(
            outcome,
            OcrGateOutcome::SkipNonText,
            "a non-textual structured doc that passes the quality check must still skip OCR"
        );
    }

    /// A genuinely scanned page (no native text layer at all) must still route to OCR,
    /// preserving pre-existing behavior alongside the new undecodable-text-layer trigger.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_scanned_empty_page_still_routes_to_ocr() {
        let decision = evaluate_native_text_for_ocr("   \n\t  ", Some(1), &t());
        assert!(decision.fallback, "an empty/scanned page must still route to OCR");
        assert_eq!(decision.stats.undecodable_ratio, 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_single_bad_page_triggers() {
        use crate::types::PageBoundary;

        let text = "Good text on page one with meaningful content.\x00\x00\x00";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 46,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 46,
                byte_end: text.len(),
            },
        ];
        let decision = evaluate_per_page_ocr(text, Some(&boundaries), Some(2), &t());
        assert!(decision.fallback);
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_empty_ocr_result_keeps_native_text() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE NATIVE\nPAGE TWO NATIVE";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 16,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 16,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(2, String::new());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert_eq!(
            merged, native,
            "an empty OCR result must not overwrite the page's native text"
        );
        assert!(merged.contains("PAGE TWO NATIVE"));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_nonempty_ocr_result_replaces_native_text() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE NATIVE\ngarbage page two";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 16,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 16,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(2, "CLEAN OCR PAGE TWO".to_string());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert!(merged.contains("PAGE ONE NATIVE"));
        assert!(merged.contains("CLEAN OCR PAGE TWO"));
        assert!(!merged.contains("garbage page two"));
    }

    #[test]
    fn test_accepted_replacements_reject_empty_missing_duplicate_overlap_and_invalid_utf8() {
        use crate::types::PageBoundary;

        let native = "A•BCDE";
        let bullet = native.find('•').unwrap();
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: native.len(),
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 3,
                byte_start: 0,
                byte_end: 1,
            },
            PageBoundary {
                page_number: 3,
                byte_start: 1,
                byte_end: 1,
            },
            PageBoundary {
                page_number: 4,
                byte_start: bullet + 1,
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 5,
                byte_start: 0,
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 6,
                byte_start: 1,
                byte_end: native.len(),
            },
        ];
        let mut raw = ahash::AHashMap::new();
        raw.insert(1, "accepted".to_string());
        raw.insert(2, "missing boundary".to_string());
        raw.insert(3, "duplicate boundary".to_string());
        raw.insert(4, "invalid UTF-8 offset".to_string());
        raw.insert(5, "overlap one".to_string());
        raw.insert(6, "overlap two".to_string());
        raw.insert(7, "   ".to_string());

        let accepted = accepted_ocr_page_replacements(native, &boundaries, &raw);

        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted.get(&1).map(String::as_str), Some("accepted"));
    }

    #[test]
    fn test_zero_width_consecutive_replacements_have_deterministic_page_order() {
        use crate::types::PageBoundary;

        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 0,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 0,
                byte_end: 0,
            },
        ];
        let raw = ahash::AHashMap::from_iter([(2, "page two".to_string()), (1, "page one|".to_string())]);

        let accepted = accepted_ocr_page_replacements("", &boundaries, &raw);
        let merged = apply_ocr_page_replacements("", &boundaries, &accepted);

        assert_eq!(merged, "page one|page two");
    }

    #[test]
    fn test_structured_mixed_merge_preserves_assets_and_remaps_relationships() {
        use crate::types::internal::{
            ElementKind, InternalDocument, InternalElement, Relationship, RelationshipKind, RelationshipTarget,
        };

        let mut doc = InternalDocument::new("pdf");
        doc.tables.push(crate::types::Table {
            cells: vec![vec!["kept".to_string()]],
            markdown: "| kept |".to_string(),
            page_number: 2,
            bounding_box: None,
            ..Default::default()
        });
        doc.images.push(crate::types::ExtractedImage {
            image_index: 0,
            page_number: Some(2),
            ocr_result: Some(Box::new(crate::types::ExtractedDocument {
                content: "DUPLICATE INLINE OCR".to_string(),
                ..Default::default()
            })),
            ..Default::default()
        });
        let mut push = |kind, text: &str, page| {
            let mut element = InternalElement::text(kind, text, 0);
            element.page = page;
            doc.push_element(element);
        };
        push(ElementKind::Paragraph, "native page one", Some(1));
        push(ElementKind::PageBreak, "", None);
        push(ElementKind::ListStart { ordered: false }, "", None);
        push(ElementKind::ListItem { ordered: false }, "stale page two", Some(2));
        push(ElementKind::Table { table_index: 0 }, "", Some(2));
        push(ElementKind::Image { image_index: 0 }, "", Some(2));
        push(ElementKind::ListEnd, "", None);
        push(ElementKind::PageBreak, "", None);
        push(ElementKind::Paragraph, "native page three", Some(3));
        doc.elements[3].anchor = Some("removed-target".to_string());
        doc.elements[8].anchor = Some("retained-target".to_string());
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Index(5),
            kind: RelationshipKind::Caption,
        });
        doc.relationships.push(Relationship {
            source: 3,
            target: RelationshipTarget::Index(8),
            kind: RelationshipKind::Caption,
        });
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Key("retained-target".to_string()),
            kind: RelationshipKind::InternalLink,
        });
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Key("removed-target".to_string()),
            kind: RelationshipKind::InternalLink,
        });

        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2, "DUPLICATE INLINE OCR\n\nOCR paragraph two".to_string());
        merge_ocr_pages_into_internal_document(&mut doc, &ocr_results);

        let kinds: Vec<ElementKind> = doc.elements.iter().map(|element| element.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| matches!(kind, ElementKind::PageBreak))
                .count(),
            2
        );
        assert!(!kinds.iter().any(|kind| matches!(kind, ElementKind::Table { .. })));
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| matches!(kind, ElementKind::Image { .. }))
                .count(),
            1
        );
        assert!(
            !doc.elements
                .iter()
                .any(|element| element.text.contains("stale page two"))
        );
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::OcrText { .. }))
                .map(|element| element.text.as_str())
                .collect::<Vec<_>>(),
            vec!["DUPLICATE INLINE OCR", "OCR paragraph two"]
        );
        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.images.len(), 1);
        assert!(
            doc.images[0].ocr_result.is_some(),
            "public nested OCR data must be preserved"
        );
        doc.append_ocr_text = true;
        for rendered in [
            crate::rendering::render_plain(&doc),
            crate::rendering::render_markdown(&doc),
            crate::rendering::render_djot(&doc),
        ] {
            assert_eq!(
                rendered.matches("DUPLICATE INLINE OCR").count(),
                1,
                "whole-page OCR must suppress duplicate nested image OCR rendering: {rendered}"
            );
        }
        let derived = crate::extraction::derive::derive_extraction_result(
            doc.clone(),
            true,
            crate::core::config::OutputFormat::Plain,
        );
        let document = serde_json::to_string(derived.document.as_ref().expect("document structure must exist"))
            .expect("document structure must serialize");
        assert!(
            !document.contains("xberg:internal"),
            "internal renderer flags must not be public"
        );
        assert_eq!(doc.relationships.len(), 2);
        let RelationshipTarget::Index(target) = doc.relationships[0].target else {
            panic!("retained indexed relationship must stay resolved");
        };
        assert!(matches!(doc.elements[target as usize].kind, ElementKind::Image { .. }));
        assert!(matches!(doc.relationships[1].target, RelationshipTarget::Key(ref key) if key == "retained-target"));
        let ids: std::collections::HashSet<&str> = doc.elements.iter().map(|element| element.id.as_ref()).collect();
        assert_eq!(ids.len(), doc.elements.len(), "rebuilt element IDs must be unique");
    }

    #[test]
    fn test_structured_mixed_merge_inserts_missing_page_in_order() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "page one", 0).with_page(1));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "page three", 0).with_page(3));
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2, "new page two".to_string());

        merge_ocr_pages_into_internal_document(&mut doc, &ocr_results);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|element| !element.text.is_empty())
            .map(|element| element.text.as_str())
            .collect();
        assert_eq!(texts, vec!["page one", "new page two", "page three"]);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::PageBreak))
                .count(),
            2
        );
    }

    #[test]
    fn test_structured_mixed_merge_prefers_page_document_and_keeps_text_fallback() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut native = InternalDocument::new("pdf");
        native.push_element(InternalElement::text(ElementKind::Paragraph, "native one", 0).with_page(1));
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale two", 0).with_page(2));
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale three", 0).with_page(3));

        let mut structured_page = InternalDocument::new("pdf");
        structured_page.push_element(
            InternalElement::text(ElementKind::Heading { level: 2 }, "Structured OCR heading", 0).with_page(1),
        );
        let empty_structured_page = InternalDocument::new("pdf");
        let structured_pages = ahash::AHashMap::from_iter([(2, structured_page), (3, empty_structured_page)]);
        let replacements =
            ahash::AHashMap::from_iter([(2, "flat OCR two".to_string()), (3, "fallback OCR three".to_string())]);

        merge_structured_ocr_pages_into_internal_document(&mut native, &replacements, &structured_pages);

        assert!(native.elements.iter().any(|element| {
            element.text == "Structured OCR heading"
                && element.page == Some(2)
                && matches!(element.kind, ElementKind::Heading { level: 2 })
        }));
        assert!(!native.elements.iter().any(|element| element.text == "flat OCR two"));
        assert!(native.elements.iter().any(|element| {
            element.text == "fallback OCR three"
                && element.page == Some(3)
                && matches!(element.kind, ElementKind::OcrText { .. })
        }));
    }

    /// A structured OCR page carrying assets is merged structurally, not flattened
    /// back to raw text (#57/#59). This previously asserted the opposite: the flat
    /// fallback ran and the page's table was lost.
    #[test]
    fn test_structured_mixed_merge_reindexes_pages_with_assets() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut native = InternalDocument::new("pdf");
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale page", 0).with_page(2));

        let mut structured_page = InternalDocument::new("pdf");
        structured_page.push_element(
            InternalElement::text(ElementKind::Heading { level: 2 }, "heading before table", 0).with_page(2),
        );
        structured_page.push_element(InternalElement::text(ElementKind::Table { table_index: 0 }, "", 0).with_page(2));
        structured_page.tables.push(crate::types::Table {
            markdown: "| value |\n| --- |\n| retained |".to_string(),
            page_number: 2,
            ..Default::default()
        });

        let structured_pages = ahash::AHashMap::from_iter([(2, structured_page)]);
        let replacements = ahash::AHashMap::from_iter([(
            2,
            "heading before table\n\n| value |\n| --- |\n| retained |".to_string(),
        )]);

        merge_structured_ocr_pages_into_internal_document(&mut native, &replacements, &structured_pages);

        assert_eq!(
            native.tables.len(),
            1,
            "the page's table must be merged into the parent"
        );
        assert_eq!(native.tables[0].markdown, "| value |\n| --- |\n| retained |");
        assert_eq!(native.tables[0].page_number, 2);
        assert!(
            native.elements.iter().any(|element| {
                element.text == "heading before table" && matches!(element.kind, ElementKind::Heading { level: 2 })
            }),
            "the structured heading must survive instead of being flattened to OCR text"
        );
        assert!(
            native
                .elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::Table { table_index: 0 })),
            "the table reference must be rebased onto the parent's collection"
        );
        assert!(
            !native
                .elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::OcrText { .. })),
            "the raw-text fallback must not run for a structurally merged page"
        );
        assert!(!native.elements.iter().any(|element| element.text == "stale page"));
    }

    #[test]
    fn test_empty_structured_page_keeps_recovered_flat_ocr_text() {
        let mut pages = vec![Some(Vec::new())];
        let page_texts = vec!["Recovered embedded image text".to_string()];

        fill_unstructured_ocr_pages(&mut pages, &page_texts);

        let paragraphs = pages[0].as_ref().expect("recovered page must be represented");
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Recovered embedded image text");
    }

    #[test]
    fn test_structured_merge_handles_first_last_consecutive_and_textless_pages() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        for page in 1..=4 {
            doc.push_element(
                InternalElement::text(ElementKind::Paragraph, format!("native {page}"), 0).with_page(page),
            );
        }
        let replacements = ahash::AHashMap::from_iter([
            (1, "same OCR".to_string()),
            (2, "same OCR".to_string()),
            (4, "last OCR".to_string()),
            (5, "textless OCR".to_string()),
        ]);

        merge_ocr_pages_into_internal_document(&mut doc, &replacements);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|element| !element.text.is_empty())
            .map(|element| element.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["same OCR", "same OCR", "native 3", "last OCR", "textless OCR"]
        );
        let ids: std::collections::HashSet<&str> = doc.elements.iter().map(|element| element.id.as_ref()).collect();
        assert_eq!(
            ids.len(),
            doc.elements.len(),
            "repeated OCR text still needs unique IDs"
        );
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::PageBreak))
                .count(),
            4
        );
    }

    #[test]
    fn test_container_analysis_keeps_only_balanced_same_page_markers() {
        use crate::types::internal::{ElementKind, InternalElement};

        let element = |kind, page| {
            let mut element = InternalElement::text(kind, "", 0);
            element.page = page;
            element
        };
        let elements = vec![
            element(ElementKind::ListStart { ordered: false }, None),
            element(ElementKind::GroupStart, Some(1)),
            element(ElementKind::Paragraph, Some(1)),
            element(ElementKind::GroupEnd, None),
            element(ElementKind::ListEnd, None),
            element(ElementKind::QuoteStart, None),
            element(ElementKind::Paragraph, Some(1)),
            element(ElementKind::Paragraph, Some(2)),
            element(ElementKind::QuoteEnd, None),
            element(ElementKind::ListEnd, None),
            element(ElementKind::GroupStart, None),
            element(ElementKind::ListStart { ordered: true }, Some(1)),
            element(ElementKind::QuoteStart, Some(1)),
            element(ElementKind::ListEnd, None),
            element(ElementKind::QuoteEnd, None),
        ];

        let analysis = analyze_container_markers(&elements);

        for index in [0, 1, 3, 4] {
            assert!(!analysis.drop_marker[index], "valid nested marker {index} must survive");
            assert_eq!(analysis.inferred_pages[index], Some(1));
        }
        for index in [5, 8, 9, 10, 11, 13] {
            assert!(analysis.drop_marker[index], "invalid marker {index} must be flattened");
        }
        assert!(
            !analysis.drop_marker[12],
            "independently balanced inner quote must survive"
        );
        assert!(
            !analysis.drop_marker[14],
            "independently balanced inner quote must survive"
        );
    }

    /// Boundaries can go stale when the text they index is rebuilt (e.g.
    /// reading-order reordering). A stale offset landing inside a multibyte
    /// character must be skipped, not panic the page.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_non_char_boundary_offsets_skipped() {
        use crate::types::PageBoundary;

        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences • that form a coherent text block.";
        let mid_bullet = text.find('•').unwrap() + 1;
        assert!(!text.is_char_boundary(mid_bullet));
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: mid_bullet,
            },
            PageBoundary {
                page_number: 2,
                byte_start: mid_bullet,
                byte_end: text.len(),
            },
        ];
        let decision = evaluate_per_page_ocr(text, Some(&boundaries), Some(2), &t());
        assert!(
            decision.failing_pages.is_empty(),
            "stale non-char-boundary offsets must be skipped, not evaluated"
        );
    }

    /// Same staleness in the mixed OCR/native merge: a boundary that does not
    /// land on char boundaries must leave the native text untouched.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_non_char_boundary_offsets_skipped() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE • NATIVE\nPAGE TWO NATIVE";
        let mid_bullet = native.find('•').unwrap() + 1;
        assert!(!native.is_char_boundary(mid_bullet));
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: mid_bullet,
            },
            PageBoundary {
                page_number: 2,
                byte_start: mid_bullet,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(1, "OCR PAGE ONE".to_string());
        ocr_results.insert(2, "OCR PAGE TWO".to_string());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert_eq!(
            merged, native,
            "stale non-char-boundary offsets must not be spliced into the native text"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_few_replacement_chars_no_fallback() {
        let text = "The quick \u{FFFD} brown fox jumps over the lazy dog repeatedly.";
        let stats = NativeTextStats::from(text);
        assert_eq!(stats.garbage_char_count, 1);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_consecutive_repeat_high_with_substantial_content_no_ocr() {
        let defaults = t();
        let mut words = Vec::new();
        for _ in 0..10 {
            words.extend_from_slice(&[
                "TALK", "TALK", "of", "of", "the", "the", "TOWN", "TOWN", "London", "London",
            ]);
        }
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(
            stats.consecutive_repeat_ratio >= defaults.min_consecutive_repeat_ratio,
            "ratio {} should be >= {}",
            stats.consecutive_repeat_ratio,
            defaults.min_consecutive_repeat_ratio
        );
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);

        assert!(
            !decision.fallback,
            "Substantial content should NOT trigger OCR even with high repeat ratio. \
             Stats: non_ws={}, avg_non_ws={:.2}",
            stats.non_whitespace, decision.avg_non_whitespace
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_no_consecutive_repeat_false_positive() {
        let defaults = t();
        let text = "The quick brown fox jumps over the lazy dog. This is a completely normal \
                    paragraph of text that forms coherent sentences. It contains multiple \
                    meaningful words and no unusual patterns of repetition. The text continues \
                    with more content that demonstrates typical English prose structure and \
                    vocabulary distribution across several sentences of varying length.";
        let stats = NativeTextStats::from(text);
        assert!(
            stats.consecutive_repeat_ratio < defaults.min_consecutive_repeat_ratio,
            "Normal text ratio {} should be < {}",
            stats.consecutive_repeat_ratio,
            defaults.min_consecutive_repeat_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_critical_fragmentation_triggers_fallback() {
        let defaults = t();
        let mut words: Vec<&str> = vec!["A"; 90];
        words.extend(vec!["document"; 10]);
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(
            stats.fragmented_word_ratio >= defaults.critical_fragmented_word_ratio,
            "fragmented ratio {} should be >= {}",
            stats.fragmented_word_ratio,
            defaults.critical_fragmented_word_ratio
        );
        assert!(stats.meaningful_words >= defaults.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);
        assert!(
            decision.fallback,
            "Critical fragmentation should trigger OCR even with meaningful words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_low_avg_word_length_triggers_fallback() {
        let defaults = t();
        let mut words: Vec<&str> = vec!["x"; 55];
        words.push("hello");
        words.push("world");
        words.push("testing");
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(stats.avg_word_length < defaults.min_avg_word_length);
        assert!(stats.word_count >= defaults.min_words_for_avg_length_check);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);
        assert!(decision.fallback, "Low avg word length should trigger OCR fallback");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_with_articles_no_false_positive() {
        let defaults = t();
        let text = "I am a fan of it. It is an old or new idea. A to do list is on my desk. \
                    He is in on it. We do go to it. I am at it. Is it so? He or I do it. \
                    The paragraph contains meaningful content with proper structure and sentences.";
        let stats = NativeTextStats::from(text);
        assert!(stats.meaningful_words >= defaults.min_meaningful_words);
        assert!(
            stats.fragmented_word_ratio < defaults.critical_fragmented_word_ratio,
            "Normal text fragmentation {} should be < {}",
            stats.fragmented_word_ratio,
            defaults.critical_fragmented_word_ratio
        );
        let decision = evaluate_native_text_for_ocr(text, Some(1), &defaults);
        assert!(
            !decision.fallback,
            "Normal text with short words should not trigger OCR"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_short_words_in_normal_text_no_false_positive() {
        let text = "I am a fan of this document. He is on to something here. \
                    We do have meaningful words like paragraph and structure throughout.";
        let stats = NativeTextStats::from(text);
        assert!(stats.meaningful_words >= t().min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_good_text() {
        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences that form a coherent text block.";
        let score = compute_quality_score(text, &t());
        assert!(score > 0.7, "Good text should score > 0.7, got {score}");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_empty_text() {
        assert_eq!(compute_quality_score("", &t()), 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_garbled_text() {
        let text = "x y z a b c d e f g h i j k l m n o p q r s t u v w";
        let score = compute_quality_score(text, &t());
        let good_score = compute_quality_score("This is a well-formed sentence with proper words and structure.", &t());
        assert!(
            score < good_score,
            "Garbled text ({score}) should score lower than good text ({good_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_meaningful_words_no_panic() {
        let mut thresholds = t();
        thresholds.min_meaningful_words = 0;
        let score = compute_quality_score("hello world", &thresholds);
        assert!(score > 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_consecutive_repeat_ratio_no_panic() {
        let mut thresholds = t();
        thresholds.min_consecutive_repeat_ratio = 0.0;
        let score = compute_quality_score("hello hello world world", &thresholds);
        assert!(score > 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_garbage_chars_no_panic() {
        let mut thresholds = t();
        thresholds.min_garbage_chars = 0;
        let score = compute_quality_score("hello world testing", &thresholds);
        assert!(score > 0.0);
        let score_with_garbage = compute_quality_score("hello \u{FFFD} world", &thresholds);
        assert!(score > score_with_garbage);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_meaningful_words_not_capped() {
        let words: Vec<&str> = vec!["programming"; 50];
        let text = words.join(" ");
        let score = compute_quality_score(&text, &t());
        let stats = NativeTextStats::compute(&text, &t());
        assert_eq!(stats.meaningful_words, 50);
        let meaningful_score = (stats.meaningful_words as f64 / t().min_meaningful_words as f64).min(1.0);
        assert!(
            (meaningful_score - 1.0).abs() < f64::EPSILON,
            "meaningful_score should be 1.0 with 50 meaningful words, got {meaningful_score}"
        );
        assert!(
            score > 0.7,
            "Score with many meaningful words should be high, got {score}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_repeat_threshold_relative_normalization() {
        let thresholds = t();
        let text = "The quick brown fox jumps over the lazy dog near the stream. \
                    The quick brown fox jumps over the lazy dog near the stream. \
                    The quick brown fox jumps over the lazy dog near the stream.";
        let stats = NativeTextStats::compute(text, &thresholds);
        if stats.consecutive_repeat_ratio > 0.0
            && stats.consecutive_repeat_ratio < thresholds.min_consecutive_repeat_ratio
        {
            let expected_repeat_score =
                1.0 - (stats.consecutive_repeat_ratio / thresholds.min_consecutive_repeat_ratio).min(1.0);
            let _ = expected_repeat_score;
        }
        let half_ratio = thresholds.min_consecutive_repeat_ratio / 2.0;
        let expected = 1.0 - (half_ratio / thresholds.min_consecutive_repeat_ratio).min(1.0);
        assert!(
            (expected - 0.5).abs() < f64::EPSILON,
            "repeat_score at half threshold should be 0.5, got {expected}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_strictly_monotonic() {
        let thresholds = t();

        let perfect_text = "This document contains comprehensive analysis of market trends \
                           and provides detailed recommendations for future investment strategies. \
                           The methodology involves rigorous statistical examination of historical \
                           data patterns across multiple economic sectors and geographical regions.";

        let good_text = "This is a normal paragraph with meaningful words and proper structure. \
                        It contains multiple sentences that form a coherent text block.";

        let mediocre_text = "ok so um the uh thing is that we like need to uh figure out what \
                            to do about the um situation or whatever it is that happened here today";

        let garbled_text = "x y z a b c d e f g h i j k l m n o p q r s t u v w x y z a b";

        let empty_text = "";

        let perfect_score = compute_quality_score(perfect_text, &thresholds);
        let good_score = compute_quality_score(good_text, &thresholds);
        let mediocre_score = compute_quality_score(mediocre_text, &thresholds);
        let garbled_score = compute_quality_score(garbled_text, &thresholds);
        let empty_score = compute_quality_score(empty_text, &thresholds);

        assert!(
            perfect_score > good_score,
            "perfect ({perfect_score}) > good ({good_score})"
        );
        assert!(
            good_score > mediocre_score,
            "good ({good_score}) > mediocre ({mediocre_score})"
        );
        assert!(
            mediocre_score > garbled_score,
            "mediocre ({mediocre_score}) > garbled ({garbled_score})"
        );
        assert!(
            garbled_score > empty_score,
            "garbled ({garbled_score}) > empty ({empty_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normalize_markdown_for_scoring_strips_structure() {
        let input = "# Heading\n\n\
                     | Col A | Col B |\n| --- | --- |\n| one | two |\n\n\
                     - bullet item\n\
                     ```\ncode fence body\n```\n\
                     **bold** and _italic_ words";
        let out = normalize_markdown_for_scoring(input);
        assert!(!out.contains('|'), "table pipes removed: {out:?}");
        assert!(!out.contains('#'), "heading hashes removed: {out:?}");
        assert!(!out.contains('*') && !out.contains('_'), "emphasis removed: {out:?}");
        assert!(!out.contains("```"), "code fence markers removed: {out:?}");
        assert!(!out.contains("---"), "table separator row removed: {out:?}");
        assert!(out.contains("Heading"), "heading text kept: {out:?}");
        assert!(out.contains("bullet item"), "list text kept: {out:?}");
        assert!(
            out.contains("bold") && out.contains("italic"),
            "emphasized words kept: {out:?}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normalize_markdown_for_scoring_strips_ordered_list_markers() {
        let input = "1. First item\n2) Second item\n12. Twelfth item\n10) Tenth item";
        let out = normalize_markdown_for_scoring(input);
        assert!(!out.contains("1."), "single-digit dot marker removed: {out:?}");
        assert!(!out.contains("2)"), "single-digit paren marker removed: {out:?}");
        assert!(!out.contains("12."), "multi-digit dot marker removed: {out:?}");
        assert!(!out.contains("10)"), "multi-digit paren marker removed: {out:?}");
        assert!(out.contains("First item"), "first item text kept: {out:?}");
        assert!(out.contains("Second item"), "second item text kept: {out:?}");
        assert!(out.contains("Twelfth item"), "twelfth item text kept: {out:?}");
        assert!(out.contains("Tenth item"), "tenth item text kept: {out:?}");
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_highest_score_keeps_max() {
        use crate::core::config::OcrPipelineSelection;

        // No current best: always replace.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            None,
            "some text",
            0.1
        ));
        // Strictly higher score replaces.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.4),
            "better text",
            0.5
        ));
        // Equal or lower score does not replace.
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.5),
            "equal text",
            0.5
        ));
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.9),
            "worse text",
            0.2
        ));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_overrides_lower_score() {
        use crate::core::config::OcrPipelineSelection;

        // A later, non-empty, lower-scoring stage still replaces a higher-scoring
        // earlier stage under `PreferLastNonEmpty` (#1341: a correct-but-lower-score
        // VLM transcription must win over a higher-scoring but garbled classical
        // result).
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            "correct vlm transcription",
            0.3
        ));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_keeps_prior_on_empty_candidate() {
        use crate::core::config::OcrPipelineSelection;

        // An empty later-stage result (e.g. a VLM that declined a destroyed page)
        // never overwrites an existing non-empty best.
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.4),
            "   ",
            0.0
        ));
        // But an empty candidate still becomes the best when there is no prior best.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            None,
            "",
            0.0
        ));
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_markdown_not_penalized() {
        // A VLM that emits correct, structured Markdown must not score materially below
        // the same prose without structure, or pipeline selection discards the richer
        // result in favor of a classical backend (#1341).
        let thresholds = t();
        let plain = "Quarterly revenue rose across every region this year. The northern \
                     division led growth while the southern division held steady and the \
                     eastern division recovered from the prior downturn this fiscal period.";
        let markdown = "## Quarterly revenue\n\n\
                        Quarterly revenue rose across every region this year.\n\n\
                        | Region | Trend |\n| --- | --- |\n| Northern | led growth |\n\
                        | Southern | held steady |\n| Eastern | recovered |\n\n\
                        - The northern division led growth this fiscal period\n\
                        - The southern division held steady while the eastern recovered";
        let plain_score = compute_quality_score(plain, &thresholds);
        let markdown_score = compute_quality_score(markdown, &thresholds);
        assert!(
            markdown_score >= plain_score - 0.05,
            "structured markdown ({markdown_score}) must not be heavily penalized vs plain prose ({plain_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_high_garbage_chars() {
        let thresholds = t();
        let text = format!("Hello world testing {} more words here", "\u{FFFD}".repeat(20));
        let score = compute_quality_score(&text, &thresholds);
        let clean_score = compute_quality_score("Hello world testing more words here", &thresholds);
        assert!(
            score < clean_score,
            "Text with garbage chars ({score}) should score lower than clean text ({clean_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_high_consecutive_repetition() {
        let thresholds = t();
        let mut words = Vec::new();
        for _ in 0..30 {
            words.push("word");
            words.push("word");
        }
        let text = words.join(" ");
        let score = compute_quality_score(&text, &thresholds);
        let normal_score = compute_quality_score(
            "The quick brown fox jumps over the lazy dog repeatedly in various ways throughout the day",
            &thresholds,
        );
        assert!(
            score < normal_score,
            "Highly repetitive text ({score}) should score lower than normal text ({normal_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_all_zeros() {
        let text = "... --- !!! @@@ ### $$$ %%% ^^^ &&& *** ((( )))";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback, "All non-alnum text should trigger fallback");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_garbage_at_threshold() {
        let thresholds = t();
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let text = format!("Some normal text with garbage {garbage} embedded here");
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Text with garbage chars at threshold should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_fragmented_few_meaningful() {
        let thresholds = t();
        let text = "I a b c d e f g h j k l m n o p q r s u";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert!(stats.fragmented_word_ratio >= thresholds.max_fragmented_word_ratio);
        assert!(stats.meaningful_words < thresholds.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Fragmented + few meaningful words should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_critical_fragmentation_with_meaningful_words() {
        let thresholds = t();
        let mut words: Vec<&str> = vec!["A"; 90];
        words.extend(vec!["document"; 10]);
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert!(stats.fragmented_word_ratio >= thresholds.critical_fragmented_word_ratio);
        assert!(stats.meaningful_words >= thresholds.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Critical fragmentation triggers fallback even with meaningful words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_low_avg_word_length() {
        let thresholds = t();
        let mut words: Vec<&str> = vec!["a"; 55];
        words.push("hello");
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert!(stats.avg_word_length < thresholds.min_avg_word_length);
        assert!(stats.word_count >= thresholds.min_words_for_avg_length_check);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Low avg word length with enough words should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_high_consecutive_repeat_sparse() {
        let thresholds = t();

        let words = vec!["x"; 50];
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);

        assert!(
            stats.word_count >= thresholds.min_words_for_repeat_check,
            "Test setup: need >= {} words for repeat check, got {}",
            thresholds.min_words_for_repeat_check,
            stats.word_count
        );
        assert!(
            stats.consecutive_repeat_ratio >= thresholds.min_consecutive_repeat_ratio,
            "Test setup: should have high repeat ratio >= {}, got {:.2}",
            thresholds.min_consecutive_repeat_ratio,
            stats.consecutive_repeat_ratio
        );
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        if decision.avg_non_whitespace < MIN_AVG_NON_WHITESPACE_TO_TRUST {
            assert!(
                decision.fallback,
                "High consecutive repeat on sparse content should trigger fallback"
            );
        } else {
            eprintln!("Text is borderline sparse: {:.2} chars", decision.avg_non_whitespace);
        }
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_non_definitive_fails_on_alnum_ratio() {
        let thresholds = t();
        let text = "a!@# b%^ c*( d_+";
        let stats = NativeTextStats::compute(text, &thresholds);
        if stats.alnum > 0 && stats.alnum_ratio < thresholds.min_alnum_ratio && stats.non_whitespace != 0 {
            let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
            assert!(
                decision.fallback,
                "Low alnum ratio should trigger fallback through non-definitive path"
            );
        }
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_text_passes_all_checks() {
        let thresholds = t();
        let text = "This is a well-structured document containing multiple meaningful sentences. \
                    The content provides detailed information about various topics including \
                    science, technology, engineering, and mathematics. Each paragraph builds \
                    upon the previous one to create a comprehensive narrative that demonstrates \
                    proper text extraction quality from the PDF document format.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
        assert!(!decision.fallback, "Well-formed text should pass all checks");
        assert!(decision.stats.meaningful_words >= thresholds.min_meaningful_words);
        assert!(decision.stats.alnum_ratio >= thresholds.min_alnum_ratio);
        assert!(decision.stats.garbage_char_count < thresholds.min_garbage_chars);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_meaningful_words_actual_count_not_capped() {
        let thresholds = t();
        let words: Vec<&str> = vec!["programming"; 20];
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert_eq!(
            stats.meaningful_words, 20,
            "meaningful_words should be 20 (not capped), got {}",
            stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_fragmented_word_ratio_calculation() {
        let thresholds = t();
        let text = "I a am b so the one quick brown fox";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert_eq!(stats.word_count, 10);
        let expected_ratio = 5.0 / 10.0;
        assert!(
            (stats.fragmented_word_ratio - expected_ratio).abs() < 0.01,
            "fragmented_word_ratio should be ~{expected_ratio}, got {}",
            stats.fragmented_word_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_fragmented_word_ratio_below_10_words() {
        let thresholds = t();
        let text = "a b c d e f g h i";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert_eq!(stats.word_count, 9);
        assert_eq!(
            stats.fragmented_word_ratio, 0.0,
            "fragmented_word_ratio should be 0.0 with < 10 words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_consecutive_repeat_ratio_calculation() {
        let thresholds = t();
        let mut words = Vec::new();
        for _ in 0..25 {
            words.push("alpha");
            words.push("beta");
        }
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert_eq!(stats.word_count, 50);
        assert!(
            stats.consecutive_repeat_ratio < 0.01,
            "Alternating words should have ~0 repeat ratio, got {}",
            stats.consecutive_repeat_ratio
        );

        let mut repeat_words = Vec::new();
        for _ in 0..25 {
            repeat_words.push("same");
            repeat_words.push("same");
        }
        let repeat_text = repeat_words.join(" ");
        let repeat_stats = NativeTextStats::compute(&repeat_text, &thresholds);
        assert!(
            repeat_stats.consecutive_repeat_ratio > 0.4,
            "All-same words should have high repeat ratio, got {}",
            repeat_stats.consecutive_repeat_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_consecutive_repeat_below_min_words() {
        let thresholds = t();
        let text = "same same same";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert!(stats.word_count < thresholds.min_words_for_repeat_check);
        assert_eq!(
            stats.consecutive_repeat_ratio, 0.0,
            "consecutive_repeat_ratio should be 0.0 below word threshold"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_empty_string() {
        let thresholds = t();
        let stats = NativeTextStats::compute("", &thresholds);
        assert_eq!(stats.non_whitespace, 0);
        assert_eq!(stats.alnum, 0);
        assert_eq!(stats.meaningful_words, 0);
        assert_eq!(stats.alnum_ratio, 0.0);
        assert_eq!(stats.garbage_char_count, 0);
        assert_eq!(stats.fragmented_word_ratio, 0.0);
        assert_eq!(stats.consecutive_repeat_ratio, 0.0);
        assert_eq!(stats.avg_word_length, 0.0);
        assert_eq!(stats.word_count, 0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_single_word() {
        let thresholds = t();
        let stats = NativeTextStats::compute("hello", &thresholds);
        assert_eq!(stats.word_count, 1);
        assert_eq!(stats.non_whitespace, 5);
        assert_eq!(stats.alnum, 5);
        assert_eq!(stats.meaningful_words, 1);
        assert_eq!(stats.avg_word_length, 5.0);
        assert_eq!(stats.fragmented_word_ratio, 0.0);
        assert_eq!(stats.consecutive_repeat_ratio, 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_single_char() {
        let thresholds = t();
        let stats = NativeTextStats::compute("x", &thresholds);
        assert_eq!(stats.word_count, 1);
        assert_eq!(stats.non_whitespace, 1);
        assert_eq!(stats.alnum, 1);
        assert_eq!(stats.meaningful_words, 0);
        assert_eq!(stats.avg_word_length, 1.0);
    }

    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_process_document_propagation() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::path::Path;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct MockBackend {
            called: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for MockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                panic!("Should not call process_image");
            }
            fn supports_document_processing(&self) -> bool {
                true
            }
            async fn process_document(&self, path: &Path, _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                assert!(path.to_string_lossy().contains("test.pdf"));
                self.called.store(true, Ordering::SeqCst);
                Ok(ExtractedDocument::default())
            }
        }

        impl Plugin for MockBackend {
            fn name(&self) -> &str {
                "mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let called = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(MockBackend { called: called.clone() });
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::plugins::register_ocr_backend(backend).unwrap();

        let path = Path::new("test.pdf");
        let result = extract_with_ocr(
            None,
            Some(&[]),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            Some(path),
        )
        .await;

        assert!(result.is_ok());
        assert!(called.load(Ordering::SeqCst), "process_document was not called");
        let (_, _, _, _, _, llm_usage, _, _, _) = result.unwrap();
        assert!(llm_usage.is_empty(), "No LLM usage expected for mock backend");

        crate::plugins::unregister_ocr_backend("mock").unwrap();
    }

    /// Verifies that `llm_usage` entries returned by a VLM OCR backend are
    /// accumulated per-page and returned from `extract_with_ocr`.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_llm_usage_propagated_through_extract_with_ocr() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, LlmUsage};
        use std::sync::Arc;

        struct VlmMockBackend;

        #[async_trait::async_trait]
        impl OcrBackend for VlmMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "page text".to_string(),
                    llm_usage: Some(vec![LlmUsage {
                        model: "gpt-4o".to_string(),
                        source: "vlm_ocr".to_string(),
                        input_tokens: Some(100),
                        output_tokens: Some(50),
                        total_tokens: Some(150),
                        estimated_cost: Some(0.001),
                        finish_reason: Some("stop".to_string()),
                    }]),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for VlmMockBackend {
            fn name(&self) -> &str {
                "vlm-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(VlmMockBackend);
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "vlm-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::plugins::register_ocr_backend(backend).unwrap();

        let tiny_png = {
            use image::ImageEncoder;
            use image::codecs::png::PngEncoder;
            use std::io::Cursor;
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = Cursor::new(Vec::new());
            PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };
        let images = vec![tiny_png.clone(), tiny_png];

        let result = extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("vlm-mock").unwrap();

        let (_, _, _, _, _, llm_usage, _, _, _) = result.expect("extract_with_ocr should succeed");
        assert_eq!(
            llm_usage.len(),
            2,
            "should have one LlmUsage entry per page, got {}",
            llm_usage.len()
        );
        assert_eq!(llm_usage[0].model, "gpt-4o");
        assert_eq!(llm_usage[0].source, "vlm_ocr");
        assert_eq!(llm_usage[0].total_tokens, Some(150));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[tokio::test]
    #[serial_test::serial]
    async fn accepted_fallback_retains_prior_stage_diagnostics() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const FAILED_BACKEND: &str = "accepted-fallback-primary-failure";
        const FALLBACK_BACKEND: &str = "accepted-fallback-success";
        const UNAVAILABLE_BACKEND: &str = "accepted-fallback-unavailable";
        const FALLBACK_TEXT: &str =
            "This readable fallback result contains enough natural language words to clear the OCR quality threshold.";

        struct FailedPrimaryBackend;
        struct AcceptedFallbackBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FailedPrimaryBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }

            fn supports_language(&self, _: &str) -> bool {
                true
            }

            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Err(crate::XbergError::Parsing {
                    message: "synthetic primary failure".to_string(),
                    source: None,
                })
            }
        }

        impl Plugin for FailedPrimaryBackend {
            fn name(&self) -> &str {
                FAILED_BACKEND
            }

            fn version(&self) -> String {
                "1.0.0".to_string()
            }

            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }

            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl OcrBackend for AcceptedFallbackBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }

            fn supports_language(&self, _: &str) -> bool {
                true
            }

            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: FALLBACK_TEXT.to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for AcceptedFallbackBackend {
            fn name(&self) -> &str {
                FALLBACK_BACKEND
            }

            fn version(&self) -> String {
                "1.0.0".to_string()
            }

            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }

            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FailedPrimaryBackend)).unwrap();
        crate::plugins::register_ocr_backend(Arc::new(AcceptedFallbackBackend)).unwrap();

        let pipeline = OcrPipelineConfig {
            stages: vec![
                OcrPipelineStage {
                    backend: FAILED_BACKEND.to_string(),
                    priority: 120,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
                OcrPipelineStage {
                    backend: UNAVAILABLE_BACKEND.to_string(),
                    priority: 110,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
                OcrPipelineStage {
                    backend: FALLBACK_BACKEND.to_string(),
                    priority: 100,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
            ],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.05,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let images = vec![image::DynamicImage::new_rgb8(16, 16)];

        let result = run_ocr_pipeline(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(FAILED_BACKEND).unwrap();
        crate::plugins::unregister_ocr_backend(FALLBACK_BACKEND).unwrap();

        let (text, _, _, doc, _, _, _, _) = result.expect("fallback stage must be accepted");
        assert_eq!(text, FALLBACK_TEXT);
        let warnings = doc
            .expect("accepted fallback diagnostics require an internal document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains(FAILED_BACKEND) && warning.message.contains("failed")),
            "primary-stage failure must survive accepted fallback: {warnings:?}"
        );
        assert!(
            warnings.iter().any(
                |warning| warning.message.contains(UNAVAILABLE_BACKEND)
                    && warning.message.contains("unavailable")
            ),
            "unavailable requested stage must be surfaced: {warnings:?}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_build_page_raster_image_fields() {
        let png_bytes = bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n");
        let img = build_page_raster_image(0, png_bytes.clone(), 800, 600);

        assert_eq!(img.page_number, Some(1), "page_number must be 1-indexed");
        assert_eq!(img.width, Some(800));
        assert_eq!(img.height, Some(600));
        assert_eq!(img.format.as_ref(), "png");
        assert_eq!(img.image_kind, Some(crate::types::ImageKind::PageRaster));
        assert_eq!(img.colorspace.as_deref(), Some("RGB"));
        assert_eq!(img.bits_per_component, Some(8));
        assert!(!img.is_mask);
        assert!(img.bounding_box.is_none());
        assert!(img.ocr_result.is_none());
        assert_eq!(img.data, png_bytes);
        assert_eq!(img.image_index, 0, "image_index is a placeholder; caller must reindex");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_build_page_raster_image_page_idx_to_page_number() {
        for page_idx in 0usize..5 {
            let img = build_page_raster_image(page_idx, bytes::Bytes::new(), 100, 100);
            assert_eq!(
                img.page_number,
                Some((page_idx + 1) as u32),
                "page_number must be page_idx + 1"
            );
        }
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_unlimited_returns_none() {
        assert_eq!(parse_cgroup_v2("max\n", "12345"), None);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_numeric_saturating_subtraction() {
        assert_eq!(parse_cgroup_v2("1000000000\n", "250000000\n"), Some(750_000_000));
        assert_eq!(parse_cgroup_v2("100", "500"), Some(0));
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_invalid_returns_none() {
        assert_eq!(parse_cgroup_v2("not-a-number", "0"), None);
        assert_eq!(parse_cgroup_v2("1000", "not-a-number"), None);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v1_unlimited_sentinel_returns_none() {
        let unlimited = usize::MAX.to_string();
        assert_eq!(parse_cgroup_v1(&unlimited, "0"), None);

        let just_under = (isize::MAX as usize - 1).to_string();
        assert!(parse_cgroup_v1(&just_under, "0").is_some());
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v1_numeric_saturating_subtraction() {
        assert_eq!(parse_cgroup_v1("2000000", "500000"), Some(1_500_000));
        assert_eq!(parse_cgroup_v1("100", "500"), Some(0));
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_extracts_kb_and_converts_to_bytes() {
        let synthetic = "\
MemTotal:        8000000 kB
MemFree:         1000000 kB
MemAvailable:       2048 kB
Buffers:           50000 kB
";
        assert_eq!(parse_meminfo_available(synthetic), 2048 * 1024);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_missing_field_returns_zero() {
        let synthetic = "MemTotal: 8000000 kB\nMemFree: 1000000 kB\n";
        assert_eq!(parse_meminfo_available(synthetic), 0);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_handles_unparseable_value_as_zero() {
        let synthetic = "MemAvailable: notanumber kB\n";
        assert_eq!(parse_meminfo_available(synthetic), 0);
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn shared_rendered_pages_preserve_order_content_and_backing_buffers() {
        let first = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap());
        let second = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(1, 1, vec![7, 8, 9]).unwrap());
        let first_pixels = first.as_bytes().as_ptr();
        let second_pixels = second.as_bytes().as_ptr();

        let shared = share_rendered_page_images(vec![(4, first), (1, second)]);

        assert_eq!(
            shared.iter().map(|(page_idx, _)| *page_idx).collect::<Vec<_>>(),
            vec![4, 1]
        );
        assert_eq!(shared[0].1.as_bytes(), &[1, 2, 3, 4, 5, 6]);
        assert_eq!(shared[1].1.as_bytes(), &[7, 8, 9]);
        assert_eq!(shared[0].1.as_bytes().as_ptr(), first_pixels);
        assert_eq!(shared[1].1.as_bytes().as_ptr(), second_pixels);

        let task_image = std::sync::Arc::clone(&shared[0].1);
        assert!(std::sync::Arc::ptr_eq(&task_image, &shared[0].1));
    }

    /// Pipeline-level test for the actual bug path in #1078 (force_ocr_pages / mixed
    /// path uses render_selected_pages_for_ocr; full force_ocr uses similar batch
    /// render in extract_with_ocr).
    /// This proves the wide PDF no longer hard-fails through the OCR render path
    /// that was crashing in production.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn test_render_selected_pages_for_ocr_wide_pdf_does_not_fail() {
        let wide_pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(20000.0, 300.0);
        let result = render_selected_pages_for_ocr(&wide_pdf, &[0]);
        assert!(
            result.is_ok(),
            "render_selected_pages_for_ocr on wide page (the #1078 bug path) should succeed via safeguard, got: {:?}",
            result.err()
        );
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn full_pdf_ocr_reuses_open_document_across_bounded_batches() {
        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let (doc, page_count, page_rotations) = open_pdf_for_full_ocr(&pdf).unwrap();

        assert_eq!(page_count, 1);
        let first_batch = render_full_pdf_ocr_batch(&doc, &page_rotations, 0..1).unwrap();
        assert_eq!(first_batch.len(), 1);
        assert_eq!(first_batch[0].0, 0);
        drop(first_batch);

        let second_batch = render_full_pdf_ocr_batch(&doc, &page_rotations, 0..1).unwrap();
        assert_eq!(second_batch.len(), 1);
        assert_eq!(second_batch[0].0, 0);
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_all_out_of_range_pages_skips_backend_lookup() {
        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let mut config = ExtractionConfig::default();
        config.ocr = Some(crate::core::config::OcrConfig {
            backend: "unregistered-test-backend".to_string(),
            ..Default::default()
        });

        let result = extract_mixed_ocr_native("native", &[], &[99], &pdf, &config, None)
            .await
            .unwrap();

        assert_eq!(result.0, "native");
        assert!(result.1.is_empty());
        assert!(result.2.is_empty());
        assert!(result.3.is_empty());
        assert!(result.4.is_none());
        assert!(result.5.is_empty());
        assert!(result.6.is_empty());
    }

    /// Root of the shared `test_documents` fixture corpus, resolved relative to this
    /// crate's manifest dir (mirrors the identically named helper in
    /// `crate::pdf::oxide::images` test module).
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn test_documents_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_documents")
    }

    /// Minimal 2-page PDF (no content streams, just two bare `/Page` objects) for
    /// tests that need `extract_mixed_ocr_native` to target a specific *later* page.
    /// Mirrors `crate::pdf::render::build_minimal_pdf_with_mediabox`, which already
    /// renders successfully with no content stream.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn build_minimal_two_page_pdf(w: f32, h: f32) -> Vec<u8> {
        let mut buf = Vec::<u8>::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let obj1_offset = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");

        let obj2_offset = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>\nendobj\n");

        let mb = format!("[0 0 {} {}]", w, h);
        let obj3_offset = buf.len();
        buf.extend_from_slice(format!("3 0 obj\n<</Type /Page /MediaBox {} /Parent 2 0 R>>\nendobj\n", mb).as_bytes());
        let obj4_offset = buf.len();
        buf.extend_from_slice(format!("4 0 obj\n<</Type /Page /MediaBox {} /Parent 2 0 R>>\nendobj\n", mb).as_bytes());

        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n");
        buf.extend_from_slice(b"0 5\n");
        buf.extend_from_slice(b"0000000000 65535 f \n");
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj3_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj4_offset).as_bytes());

        buf.extend_from_slice(b"trailer\n<</Size 5 /Root 1 0 R>>\n");
        buf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        buf
    }

    /// Regression test (review follow-up to #1341): the nested `run_ocr_pipeline`
    /// call for a single page assembles its aggregate text as if that lone image
    /// were page 1 of the document, so a configured page marker is stamped "PAGE 1"
    /// regardless of which real page is being OCR'd. When only a LATER page (page 2
    /// here) is routed through the pipeline route, the merged output must carry the
    /// raw backend text with no leaked "PAGE 1" marker from the nested call.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_later_page_pipeline_route_does_not_leak_page_one_marker() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, PageConfig};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct FixedTextBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FixedTextBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "OCR PAGE TWO CONTENT".to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FixedTextBackend {
            fn name(&self) -> &str {
                "later-page-marker-test-backend"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FixedTextBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);

        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                // An explicit pipeline (rather than `vlm_fallback`) so the test can
                // name its own mock backend instead of the hardcoded "vlm" name.
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "later-page-marker-test-backend".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    quality_thresholds: crate::core::config::OcrQualityThresholds::default(),
                }),
                ..Default::default()
            }),
            pages: Some(PageConfig {
                insert_page_markers: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[2], &pdf, &config, None)
            .await
            .unwrap();
        let merged = result.0;

        assert!(
            merged.contains("OCR PAGE TWO CONTENT"),
            "merged output must contain the OCR'd page 2 text: {merged:?}"
        );
        assert!(
            !merged.contains("PAGE 1"),
            "merged output must not leak a page-1 marker from the nested single-image pipeline call: {merged:?}"
        );
        assert!(
            merged.contains(page1_text),
            "page 1's native text must be untouched: {merged:?}"
        );

        crate::plugins::unregister_ocr_backend("later-page-marker-test-backend").unwrap();
    }

    /// Regression test (review follow-up to #1341): `ProcessingWarning`s produced by
    /// the nested `run_ocr_pipeline` call (e.g. "no stage cleared the quality
    /// threshold") must propagate out of `extract_mixed_ocr_native` instead of being
    /// silently dropped along with the per-page `InternalDocument`.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_pipeline_route_propagates_below_threshold_warning() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct LowQualityBackend;

        #[async_trait::async_trait]
        impl OcrBackend for LowQualityBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "low quality text".to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for LowQualityBackend {
            fn name(&self) -> &str {
                "below-threshold-warning-test-backend"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(LowQualityBackend)).unwrap();

        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let native_text = "native text";
        let boundaries = vec![PageBoundary {
            byte_start: 0,
            byte_end: native_text.len(),
            page_number: 1,
        }];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "below-threshold-warning-test-backend".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    // Impossible to clear: forces the best-effort fallback branch, which
                    // pushes a "scored below threshold" ProcessingWarning.
                    quality_thresholds: OcrQualityThresholds {
                        pipeline_min_quality: 1.1,
                        ..Default::default()
                    },
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(native_text, &boundaries, &[1], &pdf, &config, None)
            .await
            .unwrap();
        let warnings = result.6;

        assert!(
            !warnings.is_empty(),
            "below-threshold pipeline warnings must propagate out of extract_mixed_ocr_native"
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("quality threshold")),
            "expected a below-threshold warning, got: {warnings:?}"
        );

        crate::plugins::unregister_ocr_backend("below-threshold-warning-test-backend").unwrap();
    }

    /// #1444: `run_ocr_pipeline` has no OCR execution loop of its own -- every stage is
    /// delegated to `extract_with_ocr`, which already carries the force_ocr image-XObject
    /// fallback (#1355, lines ~2593-2630). This proves that delegation actually threads
    /// the recovered text and its warning through the pipeline route's return value: a
    /// page whose OCR text comes back blank, but whose page carries a real (decodable)
    /// image XObject, must be retried against the embedded image bytes and the recovered
    /// text must replace the blank result.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_blank_pipeline_page_and_warn_when_embedded_image_exists() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const BACKEND_NAME: &str = "pipeline-blank-recovery-test-backend";
        const RECOVERED_TEXT: &str = "RECOVERED PAGE TEXT";

        // Returns blank text on the first call (the full-page render) and recovered
        // text on every later call (the force_ocr fallback retry over embedded image
        // bytes). The fixture has exactly one page, so the call order is deterministic.
        struct BlankThenRecoverBackend {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for BlankThenRecoverBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let call_number = self.calls.fetch_add(1, Ordering::SeqCst);
                let content = if call_number == 0 {
                    String::new()
                } else {
                    RECOVERED_TEXT.to_string()
                };
                Ok(ExtractedDocument {
                    content,
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for BlankThenRecoverBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        crate::plugins::register_ocr_backend(Arc::new(BlankThenRecoverBackend { calls: calls.clone() })).unwrap();

        // Single page, exactly one embedded (DCT/JPEG) image XObject -- verified via
        // `mutool info -I` and already relied on by
        // `test_page_ocr_fallback_image_bytes_recovers_real_image` in
        // `crate::pdf::oxide::images`.
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        let pdf_bytes = std::fs::read(&pdf_path).expect("failed to read test PDF fixture");

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            // Accept the first (only) stage unconditionally so the test exercises the
            // fallback recovery in isolation, not the best-effort selection branch.
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _) = result.expect("pipeline run must succeed");
        assert_eq!(
            text, RECOVERED_TEXT,
            "recovered fallback text must replace the blank OCR result in the pipeline route"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "expected exactly one full-page OCR call and one fallback OCR call"
        );

        let warnings = doc
            .expect("the fallback warning must produce an internal document")
            .processing_warnings;
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one processing warning, got: {warnings:?}"
        );
        assert_eq!(warnings[0].source.as_ref(), "ocr");
        assert_eq!(
            warnings[0].message.as_ref(),
            "Page 1 rendered blank but contains 1 image XObject(s) the PDF rasterizer could not draw; \
             OCR was retried on the embedded image bytes."
        );
    }

    /// #1444: a blank OCR result on a page with no embedded image XObjects has nothing
    /// to recover from. The pipeline route must not fabricate content and must not emit
    /// the force_ocr fallback warning -- `page_ocr_fallback_image_bytes` degrades to an
    /// empty vec, and the reference implementation's `if !fallback_images.is_empty()`
    /// guard is a no-op in that case.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_not_warn_or_fabricate_content_when_blank_pipeline_page_has_no_embedded_images() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "pipeline-no-image-blank-test-backend";

        struct AlwaysBlankBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysBlankBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument::default())
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysBlankBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysBlankBackend)).unwrap();

        // No content stream, no resources, no image XObjects.
        let pdf_bytes = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _) = result.expect("pipeline run must succeed");
        assert_eq!(text, "", "a page with no recoverable images must not fabricate content");
        assert!(
            doc.is_none(),
            "a page with no recoverable images must not produce an internal document, got: {doc:?}"
        );
    }

    /// #1444: the force_ocr fallback warning is deliberately unconditional on recovery
    /// succeeding (reference implementation, ocr.rs ~2618-2630) -- it fires whenever the
    /// page had image XObjects worth retrying, even if the retry also comes back blank.
    /// The defining property of the original bug is silence, so the warning is the part
    /// that must never be skipped, independent of whether any text was recovered.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_warn_when_pipeline_fallback_ocr_recovers_nothing() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "pipeline-unrecoverable-blank-test-backend";

        struct AlwaysBlankBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysBlankBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument::default())
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysBlankBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysBlankBackend)).unwrap();

        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        let pdf_bytes = std::fs::read(&pdf_path).expect("failed to read test PDF fixture");

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _) = result.expect("pipeline run must succeed");
        assert_eq!(
            text, "",
            "an unrecovered blank page must not fabricate content even though the warning fires"
        );

        let warnings = doc
            .expect("the fallback warning must produce an internal document even with no recovered text")
            .processing_warnings;
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one processing warning, got: {warnings:?}"
        );
        assert_eq!(warnings[0].source.as_ref(), "ocr");
        assert_eq!(
            warnings[0].message.as_ref(),
            "Page 1 rendered blank but contains 1 image XObject(s) the PDF rasterizer could not draw; \
             OCR was retried on the embedded image bytes."
        );
    }

    /// Verifies that formulas returned by a per-page OCR backend are accumulated and
    /// renumbered to 1-indexed document page numbers by `extract_with_ocr`.
    ///
    /// This exercises the same `formula.page = (page_idx + 1) as u32` accumulation
    /// logic that is now replicated in `extract_mixed_ocr_native` for the mixed-OCR
    /// path. Since `extract_mixed_ocr_native` requires real PDF bytes for rendering,
    /// this test uses `extract_with_ocr` with in-memory images to validate that the
    /// accumulation pattern works correctly end-to-end.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_formulas_accumulated_and_renumbered_per_page() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{BoundingBox, ExtractedDocument};
        use std::sync::Arc;

        struct FormulaMockBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FormulaMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "page text".to_string(),
                    formulas: vec![crate::types::Formula {
                        latex: "E = mc^2".to_string(),
                        bbox: Some(BoundingBox {
                            x0: 0.0,
                            y0: 0.0,
                            x1: 100.0,
                            y1: 50.0,
                        }),
                        page: None,
                    }],
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FormulaMockBackend {
            fn name(&self) -> &str {
                "formula-mock-mixed-ocr"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(FormulaMockBackend);
        crate::plugins::register_ocr_backend(backend).unwrap();

        let tiny_image = {
            use image::ImageEncoder;
            use image::codecs::png::PngEncoder;
            use std::io::Cursor;
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = Cursor::new(Vec::new());
            PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };
        let images = vec![tiny_image.clone(), tiny_image];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "formula-mock-mixed-ocr".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("formula-mock-mixed-ocr").unwrap();

        let (_, _, _, _, _, _, _, _, formulas) = result.expect("extract_with_ocr should succeed");

        assert_eq!(formulas.len(), 2, "one formula per page, got {}", formulas.len());

        let mut pages: Vec<u32> = formulas
            .iter()
            .map(|f| f.page.expect("OCR formulas must have a renumbered page"))
            .collect();
        pages.sort_unstable();
        assert_eq!(
            pages,
            vec![1, 2],
            "formula pages must be renumbered to 1-indexed doc pages"
        );

        assert!(
            formulas.iter().all(|f| f.latex == "E = mc^2"),
            "formula latex must be preserved through accumulation"
        );
    }

    /// Test that inject_layout_config_to_backend handles non-object backend_options
    /// by replacing with a fresh object instead of silently dropping the flag.
    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn test_inject_layout_config_handles_non_object_backend_options() {
        use crate::core::config::LayoutDetectionConfig;
        let ocr_config = crate::core::config::OcrConfig {
            backend_options: Some(serde_json::json!("invalid")),
            ..Default::default()
        };

        let extraction_config = ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                enable_chart_understanding: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = inject_layout_config_to_backend(&ocr_config, &extraction_config);

        assert!(result.backend_options.is_some());
        let opts = result.backend_options.unwrap();
        assert!(opts.is_object());
        assert_eq!(
            opts.get("enable_chart_understanding").and_then(|v| v.as_bool()),
            Some(true),
            "enable_chart_understanding should be injected into the new object"
        );
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn layout_ocr_config_should_force_word_elements_for_internal_consumers() {
        let config = crate::core::config::OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: false,
                min_level: crate::types::OcrElementLevel::Line,
                min_confidence: 0.75,
                build_hierarchy: true,
            }),
            ..Default::default()
        };

        let result = ensure_elements_enabled(&config);
        let element_config = result.element_config.expect("layout OCR must request elements");

        assert!(element_config.include_elements);
        assert_eq!(element_config.min_level, crate::types::OcrElementLevel::Word);
        assert_eq!(element_config.min_confidence, 0.75);
        assert!(element_config.build_hierarchy);
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn public_ocr_elements_should_preserve_requested_granularity_and_confidence() {
        let elements = vec![
            test_ocr_element("word", crate::types::OcrElementLevel::Word, 0.9),
            test_ocr_element("weak word", crate::types::OcrElementLevel::Word, 0.4),
            test_ocr_element("line", crate::types::OcrElementLevel::Line, 0.8),
            test_ocr_element("block", crate::types::OcrElementLevel::Block, 0.95),
            test_ocr_element("page", crate::types::OcrElementLevel::Page, 0.95),
        ];
        let no_elements = crate::core::config::OcrConfig::default();
        let line_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Line, 0.5);
        let word_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Word, 0.5);
        let block_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Block, 0.0);
        let page_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Page, 0.0);

        assert!(filter_public_ocr_elements(&elements, &no_elements).is_empty());
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &line_elements)),
            vec!["line", "block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &word_elements)),
            vec!["word", "line", "block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &block_elements)),
            vec!["block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &page_elements)),
            vec!["page"]
        );
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn test_ocr_element(
        text: &str,
        level: crate::types::OcrElementLevel,
        recognition: f64,
    ) -> crate::types::OcrElement {
        crate::types::OcrElement {
            text: text.to_string(),
            level,
            confidence: crate::types::OcrConfidence {
                detection: None,
                recognition,
            },
            ..Default::default()
        }
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn ocr_config_requesting_elements(
        min_level: crate::types::OcrElementLevel,
        min_confidence: f64,
    ) -> crate::core::config::OcrConfig {
        crate::core::config::OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: true,
                min_level,
                min_confidence,
                build_hierarchy: true,
            }),
            ..Default::default()
        }
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn element_texts(elements: Vec<crate::types::OcrElement>) -> Vec<String> {
        elements.into_iter().map(|element| element.text).collect()
    }

    /// Simulate NICS background checks table: many short numeric tokens.
    /// Characteristics:
    /// - Substantial non-whitespace content (1000+ chars)
    /// - Many short numeric tokens (1-4 chars, e.g., "0", "100", "500")
    /// - High fragmented_word_ratio (~70%)
    /// - Low avg_word_length (~2.5)
    /// - High consecutive_repeat_ratio (repeated numbers)
    #[cfg(feature = "ocr")]
    fn numeric_table_text() -> String {
        let mut text = String::new();
        for row in 0..20 {
            for col in 0..15 {
                let val = (row * col) % 1000;
                text.push_str(&format!("{} ", val));
            }
            text.push('\n');
        }
        text
    }

    /// Simulate math formula page: mix of words and short tokens.
    /// Real formula pages have "where", "define", "equation", "therefore" mixed with symbols.
    /// Characteristics:
    /// - Mixture of long and short tokens
    /// - Substantial content if multiple equations
    /// - Some fragmentation from mathematical notation
    /// - But not extreme critical fragmentation (< 0.80)
    #[cfg(feature = "ocr")]
    fn formula_text() -> String {
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!(
                "Definition {}: where variable equals expression and function applies therefore x y z\n",
                i
            ));
        }
        text
    }

    /// Simulate sparse form with short tokens: checkboxes, small fields.
    /// Characteristics:
    /// - Few non-whitespace chars (<30 per page, genuinely sparse)
    /// - Short tokens
    /// - Should trigger OCR (legitimately sparse, not just non-prose)
    #[cfg(feature = "ocr")]
    fn sparse_form_text() -> String {
        let text = r#"
[]  Yes
[]  No

Name: ___
"#;
        text.to_string()
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_numeric_table_with_short_tokens_no_ocr() {
        let text = numeric_table_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            stats.non_whitespace >= 300,
            "Test setup: numeric table should have 300+ non-whitespace chars, got {}",
            stats.non_whitespace
        );
        assert!(
            decision.avg_non_whitespace >= 100.0,
            "Test setup: numeric table should have avg_non_whitespace >= 100, got {:.2}",
            decision.avg_non_whitespace
        );

        assert!(
            stats.fragmented_word_ratio > 0.5,
            "Test setup: numeric table should have high fragmentation (>0.5), got {:.2}",
            stats.fragmented_word_ratio
        );

        assert!(
            !decision.fallback,
            "Numeric table with substantial content should NOT trigger OCR fallback. \
             Stats: non_ws={}, avg_word_len={:.2}, frag_ratio={:.2}",
            stats.non_whitespace, stats.avg_word_length, stats.fragmented_word_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_formula_page_with_short_tokens_no_ocr() {
        let text = formula_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            stats.non_whitespace >= 500,
            "Test setup: formula text should have 500+ non-whitespace chars, got {}",
            stats.non_whitespace
        );

        let would_trigger_old_logic = stats.fragmented_word_ratio >= thresholds.max_fragmented_word_ratio
            && stats.meaningful_words < thresholds.min_meaningful_words;

        assert!(
            !decision.fallback,
            "Formula page with substantial content should NOT trigger OCR fallback. \
             Would trigger old logic: {}, frag={:.2}, meaningful={}",
            would_trigger_old_logic, stats.fragmented_word_ratio, stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_sparse_form_triggers_ocr() {
        let text = sparse_form_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        eprintln!(
            "Sparse form stats: non_ws={}, avg_non_ws={:.2}, meaningful_words={}, fallback={}",
            stats.non_whitespace, decision.avg_non_whitespace, stats.meaningful_words, decision.fallback
        );

        assert!(
            stats.non_whitespace < 100,
            "Test setup: sparse form should have <100 non-whitespace chars, got {}",
            stats.non_whitespace
        );

        assert!(
            decision.fallback,
            "Sparse form (legitimately few chars) SHOULD trigger OCR fallback. Stats: non_ws={}, meaningful={}",
            stats.non_whitespace, stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_short_token_dense_content_no_ocr() {
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!("Row{} ", i));

            for j in 0..15 {
                let val = (i * 13 + j * 7) % 5000;
                text.push_str(&format!("{} ", val));
            }
            text.push('\n');
        }

        let thresholds = t();
        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            decision.avg_non_whitespace >= 100.0,
            "Test setup: should have avg_non_whitespace >= 100, got {:.2}",
            decision.avg_non_whitespace
        );
        assert!(
            stats.fragmented_word_ratio < 0.80,
            "Test setup: should be sub-critical < 0.80, got {:.2}",
            stats.fragmented_word_ratio
        );

        assert!(
            !decision.fallback,
            "Dense numeric table should NOT trigger OCR fallback"
        );
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_layout_dimensions_use_valid_processed_image_metadata() {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(2000),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(3000),
        );

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (2000, 3000));
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_layout_dimensions_fall_back_for_incomplete_or_invalid_metadata() {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(0),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(3000),
        );

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (1000, 1500));

        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(2000),
        );
        metadata
            .additional
            .remove(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY);

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (1000, 1500));
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn detection_scaling_targets_ocr_coordinate_space() {
        let detection = crate::layout::DetectionResult {
            page_width: 1000,
            page_height: 1500,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.9,
                bbox: crate::layout::BBox {
                    x1: 100.0,
                    y1: 200.0,
                    x2: 400.0,
                    y2: 300.0,
                },
            }],
        };

        let scaled = scale_detection_to_dimensions(&detection, 2000, 3000);

        assert_eq!(scaled.page_width, 2000);
        assert_eq!(scaled.page_height, 3000);
        assert_eq!(scaled.detections[0].bbox.x1, 200.0);
        assert_eq!(scaled.detections[0].bbox.y1, 400.0);
        assert_eq!(scaled.detections[0].bbox.x2, 800.0);
        assert_eq!(scaled.detections[0].bbox.y2, 600.0);
    }

    /// #665: the mixed OCR route (`--ocr-scanned-pages` / `force_ocr_pages` / the per-page
    /// fallback) never threaded layout detections down into per-page OCR assembly at all --
    /// `extract_with_ocr_for_page`'s `layout_detections` parameter was hardcoded to `None` on
    /// this route, so `--layout` alone produced byte-identical output with zero layout log
    /// lines. `detection_for_mixed_route_page` is the lookup that closes that gap: given the
    /// whole-document layout pass's per-page `Vec<DetectionResult>`, it returns exactly the
    /// entry for a page's own document-wide 0-based index, unmodified.
    ///
    /// Fails on unfixed code two ways: (1) `detection_for_mixed_route_page` does not exist on
    /// the mixed route at all before this fix, so this fails to compile; (2) if the fix's
    /// per-page alignment regresses to an off-by-one (e.g. reading `page_idx + 1` or
    /// `page_idx - 1`), the middle assertion below fails because it would return page 0's or
    /// page 2's very different `page_width`/`page_height`/bbox values instead of page 1's.
    #[cfg(all(
        feature = "layout-detection",
        any(feature = "ocr", feature = "ocr-pipeline"),
        feature = "pdf"
    ))]
    #[test]
    fn detection_for_mixed_route_page_selects_the_aligned_page_without_transforming_it() {
        let page0 = crate::layout::DetectionResult {
            page_width: 111,
            page_height: 222,
            detections: Vec::new(),
        };
        let page1 = crate::layout::DetectionResult {
            page_width: 333,
            page_height: 444,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.95,
                bbox: crate::layout::BBox {
                    x1: 10.0,
                    y1: 20.0,
                    x2: 30.0,
                    y2: 40.0,
                },
            }],
        };
        let page2 = crate::layout::DetectionResult {
            page_width: 555,
            page_height: 666,
            detections: Vec::new(),
        };
        let detections = vec![page0, page1, page2];

        let found = detection_for_mixed_route_page(Some(&detections), 1).expect("page index 1 must have a detection");

        // Pinned to page 1's exact pixel-space numbers -- not page 0's or page 2's, and not a
        // rescaled derivative of them. Any coordinate transform belongs downstream, inside
        // `extract_with_ocr_for_page` (`scale_detection_to_dimensions` /
        // `scale_detection_to_ocr_coordinates`), not in this lookup.
        assert_eq!(found.page_width, 333);
        assert_eq!(found.page_height, 444);
        assert_eq!(found.detections.len(), 1);
        assert_eq!(found.detections[0].bbox.x1, 10.0);
        assert_eq!(found.detections[0].bbox.y1, 20.0);
        assert_eq!(found.detections[0].bbox.x2, 30.0);
        assert_eq!(found.detections[0].bbox.y2, 40.0);

        assert!(
            detection_for_mixed_route_page(Some(&detections), 5).is_none(),
            "an out-of-range page index must not silently return a neighboring page's detection"
        );
        assert!(
            detection_for_mixed_route_page(None, 1).is_none(),
            "no whole-document layout pass ran (e.g. layout not configured) must mean no detection for any page"
        );
    }

    /// #665: on the mixed OCR route, layout is only reachable when the single configured
    /// backend is wrapped in a one-stage pipeline and driven through `run_ocr_pipeline_for_page`
    /// -- the only per-page entry point that accepts `layout_detections` at all. Before this
    /// fix, `single_stage_pipeline_for_layout` did not exist, so this fails to compile on
    /// unfixed code. Pins the synthesized stage's fields (particularly `backend` and
    /// `language`, which the fix must copy from the real `OcrConfig` rather than defaulting)
    /// and the derived quality thresholds, since a wrong backend name here would silently
    /// route pages to the wrong OCR engine while still calling it "layout enabled".
    #[cfg(all(
        feature = "layout-detection",
        any(feature = "ocr", feature = "ocr-pipeline"),
        feature = "pdf"
    ))]
    #[test]
    fn single_stage_pipeline_for_layout_wraps_the_configured_backend() {
        let ocr_config = crate::core::config::OcrConfig {
            backend: "paddleocr".to_string(),
            language: vec!["deu".to_string()],
            ..Default::default()
        };

        let pipeline = single_stage_pipeline_for_layout(&ocr_config);

        assert_eq!(
            pipeline.stages.len(),
            1,
            "must wrap the single configured backend in exactly one stage"
        );
        assert_eq!(pipeline.stages[0].backend, "paddleocr");
        assert_eq!(pipeline.stages[0].priority, 100);
        assert_eq!(pipeline.stages[0].language, Some(vec!["deu".to_string()]));
        assert!(
            (pipeline.quality_thresholds.pipeline_min_quality - ocr_config.effective_thresholds().pipeline_min_quality)
                .abs()
                < f64::EPSILON,
            "must use the ocr config's own effective thresholds rather than an arbitrary default"
        );
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    fn rotated_ocr_metadata(final_width: u32, final_height: u32, orientation_degrees: i32) -> crate::types::Metadata {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(final_width),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(final_height),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY.into(),
            serde_json::json!(true),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY.into(),
            serde_json::json!(orientation_degrees),
        );
        metadata
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    fn rotation_test_detection() -> crate::layout::DetectionResult {
        crate::layout::DetectionResult {
            page_width: 100,
            page_height: 200,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.9,
                bbox: crate::layout::BBox {
                    x1: 10.0,
                    y1: 20.0,
                    x2: 30.0,
                    y2: 60.0,
                },
            }],
        }
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_detection_rotation_matches_clockwise_90_pixel_transform() {
        let metadata = rotated_ocr_metadata(200, 100, 270);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 100));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (140.0, 10.0, 180.0, 30.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn ocr_detection_rotation_matches_180_pixel_transform() {
        let metadata = rotated_ocr_metadata(100, 200, 180);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (100, 200));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (70.0, 140.0, 90.0, 180.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn ocr_detection_rotation_matches_clockwise_270_pixel_transform() {
        let metadata = rotated_ocr_metadata(200, 100, 90);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 100));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (20.0, 70.0, 60.0, 90.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn invalid_rotation_metadata_preserves_dimension_only_fallback() {
        let metadata = rotated_ocr_metadata(200, 400, 45);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 400));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (20.0, 40.0, 60.0, 120.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn rotated_ocr_elements_transform_back_to_render_coordinates() {
        let cases = [
            (270, (140, 10, 40, 20)),
            (180, (70, 140, 20, 40)),
            (90, (20, 70, 40, 20)),
        ];

        for (orientation, (left, top, width, height)) in cases {
            let metadata = if orientation == 180 {
                rotated_ocr_metadata(100, 200, orientation)
            } else {
                rotated_ocr_metadata(200, 100, orientation)
            };
            let element = crate::types::OcrElement {
                text: "heading".to_string(),
                geometry: crate::types::OcrBoundingGeometry::Rectangle {
                    left,
                    top,
                    width,
                    height,
                },
                ..Default::default()
            };

            let transformed = transform_ocr_elements_to_render_space(&[element], &metadata, 100, 200);

            assert_eq!(
                transformed[0].geometry,
                crate::types::OcrBoundingGeometry::Rectangle {
                    left: 10,
                    top: 20,
                    width: 20,
                    height: 40,
                },
                "orientation {orientation}"
            );
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn invalid_ocr_element_metadata_preserves_original_geometry() {
        let metadata = rotated_ocr_metadata(200, 400, 45);
        let element = crate::types::OcrElement {
            text: "heading".to_string(),
            geometry: crate::types::OcrBoundingGeometry::Rectangle {
                left: 20,
                top: 40,
                width: 60,
                height: 80,
            },
            ..Default::default()
        };

        let transformed = transform_ocr_elements_to_render_space(std::slice::from_ref(&element), &metadata, 100, 200);

        assert_eq!(transformed[0].geometry, element.geometry);
    }

    // ---------------------------------------------------------------------
    // #57 / #59 / #60 — the mixed PDF OCR path must not drop what it rebuilds.
    // ---------------------------------------------------------------------

    /// Native two-page document: page 1 native prose, page 2 native prose.
    fn native_two_page_document() -> crate::types::internal::InternalDocument {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        doc.mime_type = "application/pdf".to_string();
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "native page one", 0).with_page(1));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "native page two", 0).with_page(2));
        doc
    }

    fn ocr_table(markdown: &str, page_number: u32) -> crate::types::Table {
        crate::types::Table {
            cells: vec![vec!["a".to_string(), "b".to_string()]],
            markdown: markdown.to_string(),
            page_number,
            bounding_box: None,
            ..Default::default()
        }
    }

    /// Structured OCR result for one page: a paragraph plus a table it references.
    fn structured_ocr_page_with_table(page: u32) -> crate::types::internal::InternalDocument {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        doc.push_element(
            InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                "ocr prose",
                0,
            )
            .with_page(page),
        );
        let table_index = doc.push_table(ocr_table("| a | b |", page));
        doc.push_element(InternalElement::text(ElementKind::Table { table_index }, "", 0).with_page(page));
        doc
    }

    /// #57 — a table recognised on an OCR-replaced page must survive the merge into
    /// the parent document, both as a `tables` entry and as a referencing element.
    #[test]
    fn should_keep_ocr_page_tables_when_page_is_replaced_by_ocr() {
        use crate::types::internal::ElementKind;

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, structured_ocr_page_with_table(2));

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 1, "the OCR'd page's table must survive the merge");
        assert_eq!(doc.tables[0].markdown, "| a | b |");
        assert_eq!(doc.tables[0].page_number, 2);

        let table_indices: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|element| match element.kind {
                ElementKind::Table { table_index } => Some(table_index),
                _ => None,
            })
            .collect();
        assert_eq!(
            table_indices,
            vec![0],
            "exactly one table element, re-indexed into the parent's table collection"
        );
        assert!(
            doc.elements.iter().any(|element| element.text == "ocr prose"),
            "the structured OCR text must be used, not the raw-text fallback"
        );
        assert!(
            !doc.elements.iter().any(|element| element.text == "native page two"),
            "the replaced page's native prose must be gone"
        );
    }

    /// #59 — page assets are re-indexed against the parent's collections instead of
    /// falling back to splitting raw text, so the asset-to-page association survives.
    #[test]
    fn should_reindex_ocr_page_assets_against_parent_collections() {
        use crate::types::internal::ElementKind;

        let mut doc = native_two_page_document();
        // Parent already owns one table and one image; the OCR page's assets must be
        // appended after them, and their references rebased accordingly.
        doc.push_table(ocr_table("| pre-existing |", 1));
        doc.push_image(crate::types::ExtractedImage {
            image_index: 0,
            page_number: Some(1),
            ..Default::default()
        });

        let mut page_doc = structured_ocr_page_with_table(2);
        let image_index = page_doc.push_image(crate::types::ExtractedImage {
            image_index: 0,
            page_number: None,
            ..Default::default()
        });
        page_doc.push_element(
            crate::types::internal::InternalElement::text(ElementKind::Image { image_index }, "", 0).with_page(2),
        );

        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 2);
        assert_eq!(doc.tables[0].markdown, "| pre-existing |");
        assert_eq!(doc.tables[1].markdown, "| a | b |");
        assert_eq!(doc.images.len(), 2);
        assert_eq!(doc.images[1].image_index, 1, "merged image must be re-indexed to 1");
        assert_eq!(
            doc.images[1].page_number,
            Some(2),
            "merged image must stay associated with its OCR page"
        );

        let merged_table_index = doc.elements.iter().find_map(|element| match element.kind {
            ElementKind::Table { table_index } => Some(table_index),
            _ => None,
        });
        let merged_image_index = doc.elements.iter().find_map(|element| match element.kind {
            ElementKind::Image { image_index } => Some(image_index),
            _ => None,
        });
        assert_eq!(
            merged_table_index,
            Some(1),
            "table reference rebased onto parent index 1"
        );
        assert_eq!(
            merged_image_index,
            Some(1),
            "image reference rebased onto parent index 1"
        );
    }

    /// #59 — a page document carrying a table that its own element list never
    /// references still contributes a reference, so the table is reachable.
    #[test]
    fn should_emit_reference_for_unreferenced_ocr_page_table() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut page_doc = InternalDocument::new("pdf");
        page_doc.push_element(
            InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                "ocr prose",
                0,
            )
            .with_page(2),
        );
        page_doc.push_table(ocr_table("| orphan |", 2));

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.tables[0].markdown, "| orphan |");
        let table_indices: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|element| match element.kind {
                ElementKind::Table { table_index } => Some(table_index),
                _ => None,
            })
            .collect();
        assert_eq!(table_indices, vec![0]);
    }

    /// #60 — `prebuilt_ocr_elements` carried by an OCR page reach the parent document.
    #[test]
    fn should_carry_ocr_page_elements_into_parent_document() {
        let mut page_doc = structured_ocr_page_with_table(2);
        page_doc.prebuilt_ocr_elements = Some(vec![crate::types::OcrElement {
            text: "word".to_string(),
            page_number: 1,
            ..Default::default()
        }]);

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        let elements = doc.prebuilt_ocr_elements.expect("OCR elements must reach the parent");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "word");
        assert_eq!(elements[0].page_number, 2, "element must be renumbered onto its page");
    }

    /// #60 — the single-backend mixed route must carry the backend's tables and OCR
    /// elements onto the page document instead of discarding them.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_collect_backend_tables_and_elements_on_mixed_ocr_page() {
        let mut result = crate::types::ExtractedDocument {
            content: "scanned prose".to_string(),
            tables: vec![ocr_table("| x | y |", 0)],
            ocr_elements: Some(vec![crate::types::OcrElement {
                text: "word".to_string(),
                page_number: 1,
                ..Default::default()
            }]),
            ..Default::default()
        };

        let (page_doc, _paragraphs) = build_mixed_ocr_page_document(&mut result, 3, 1000, 1000, 1000.0, 1000.0)
            .expect("a backend result with tables must produce a page document");

        assert_eq!(page_doc.tables.len(), 1, "backend table must be kept");
        assert_eq!(page_doc.tables[0].markdown, "| x | y |");
        assert_eq!(page_doc.tables[0].page_number, 3, "table renumbered onto its page");
        let elements = page_doc
            .prebuilt_ocr_elements
            .expect("backend OCR elements must be kept");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "word");
        assert_eq!(elements[0].page_number, 3);
        assert!(
            result.tables.is_empty() && result.ocr_elements.is_none(),
            "payload is moved, not copied"
        );
    }

    /// #60 — a backend result with nothing structured keeps the previous behaviour:
    /// no page document, so the raw-text replacement path still applies.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_not_fabricate_page_document_when_backend_returns_only_text() {
        let mut result = crate::types::ExtractedDocument {
            content: "scanned prose".to_string(),
            ..Default::default()
        };

        assert!(build_mixed_ocr_page_document(&mut result, 3, 1000, 1000, 1000.0, 1000.0).is_none());
    }

    /// #633 — a backend that auto-rotated its input image (`OcrConfig::auto_rotate`)
    /// before OCR reports `ocr_internal_document` bboxes in that ROTATED image's pixel
    /// space, not the original raster the caller rendered and will rescale from
    /// (`render_width`/`render_height`). `undo_auto_rotate_document_bboxes` must map
    /// them back before `rescale_ocr_bboxes_to_page_points` runs, or the pixel->point
    /// scale divides the rotated-space bbox by the wrong (un-swapped) axis.
    ///
    /// This fails against the unfixed code (no call to
    /// `undo_auto_rotate_document_bboxes`): skipping straight to the rescale leaves
    /// the raw bbox {10, 20, 30, 60} scaled 1:1 and unchanged, not the
    /// rotation-corrected {40, 10, 80, 30} asserted below.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_undo_auto_rotate_bboxes_before_rescaling_to_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        // The backend rotated a 200x100 raster 90 degrees before OCR, producing a
        // 100x200 processed image (`OCR_ORIENTATION_DEGREES_METADATA_KEY: 90`).
        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "word",
            0,
        );
        // Pixel-space bbox in the ROTATED 100x200 processed image.
        element.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 60.0,
        });
        doc.push_element(element);

        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY),
            serde_json::Value::Bool(true),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY),
            serde_json::json!(90),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY),
            serde_json::json!(100),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY),
            serde_json::json!(200),
        );

        // Original (pre-rotation) 200x100 raster.
        undo_auto_rotate_document_bboxes(&mut doc, &metadata, 200, 100);

        // A 200x100pt page so the pixel->point rescale is 1:1 and does not obscure
        // the rotation fix.
        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 200, 100, 200.0, 100.0);

        let bbox = doc.elements[0].bbox.expect("element must keep its bbox");
        assert_eq!(bbox.x0, 40.0);
        assert_eq!(bbox.y0, 10.0);
        assert_eq!(bbox.x1, 80.0);
        assert_eq!(bbox.y1, 30.0);
    }

    /// #633 — a backend that never auto-rotated (the overwhelmingly common case,
    /// including every non-PaddleOCR backend and Paddle with `auto_rotate: false`)
    /// must be left completely unaffected: no `auto_rotated` metadata key at all.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_leave_bboxes_unchanged_when_backend_did_not_auto_rotate() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "word",
            0,
        );
        element.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 60.0,
        });
        doc.push_element(element);

        let metadata = crate::types::Metadata::default();
        undo_auto_rotate_document_bboxes(&mut doc, &metadata, 200, 100);

        let bbox = doc.elements[0].bbox.expect("element must keep its bbox");
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 30.0);
        assert_eq!(bbox.y1, 60.0);
    }

    /// #1423 — element bboxes are rescaled pixel->point (still top-left) so the later
    /// `pdf_block_bbox` flip (which now receives the page height in points) lands on
    /// exact PDF coordinates instead of raw Tesseract raster pixels.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_scales_element_bbox_without_flipping() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        // Pixel-space, top-left origin: y0 is the box's top row, y1 its bottom row.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });
        doc.push_element(element);

        // 1700x2200px raster of a 612x792pt (US Letter) page: scale_x = scale_y = 0.36.
        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 1700, 2200, 612.0, 792.0);

        let bbox = doc.elements[0].bbox.expect("bbox must survive rescale");
        assert_eq!(bbox.x0, 36.0);
        assert_eq!(bbox.y0, 72.0);
        assert_eq!(bbox.x1, 108.0);
        assert_eq!(bbox.y1, 144.0);
    }

    /// #1423 — table bboxes get the full pixel->point conversion *and* the top-left ->
    /// bottom-left flip here, since nothing downstream flips them (`push_table_element`
    /// copies `Table::bounding_box` through unchanged). The result must match the
    /// bottom-left/points contract documented on `Table::bounding_box`: a box near the
    /// top of the page ends up with a y1 (top) close to `page_height_pt`, not close to 0.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_scales_and_flips_table_bbox() {
        use crate::types::extraction::BoundingBox;

        let mut tables = [ocr_table("| a | b |", 0)];
        // `convert_ocr_table` stores the raw pixel rect as {x0: left, y0: top, x1:
        // right, y1: bottom} — top-left origin, unscaled pixels.
        tables[0].bounding_box = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });

        rescale_ocr_bboxes_to_page_points(None, &mut tables, 1700, 2200, 612.0, 792.0);

        let bbox = tables[0].bounding_box.expect("bbox must survive rescale");
        assert_eq!(bbox.x0, 36.0, "left edge scales by scale_x");
        assert_eq!(bbox.x1, 108.0, "right edge scales by scale_x");
        assert_eq!(bbox.y0, 648.0, "bottom = page_height_pt - bottom_px * scale_y");
        assert_eq!(bbox.y1, 720.0, "top = page_height_pt - top_px * scale_y");
        assert!(bbox.y0 < bbox.y1, "bottom-left origin: y0 (bottom) must be < y1 (top)");
    }

    /// #1423 — zero raster dimensions (e.g. a synthetic document with no rendered page
    /// behind it) must leave bboxes untouched rather than dividing by zero or
    /// fabricating a scale factor.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_is_a_noop_when_image_dimensions_are_zero() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        let original = BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        };
        element.bbox = Some(original);
        doc.push_element(element);

        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 0, 0, 612.0, 792.0);

        assert_eq!(doc.elements[0].bbox, Some(original));
    }

    /// #1423 end-to-end: the single-backend mixed OCR route must hand
    /// `assemble_mixed_ocr_page_document` bboxes already in the page's point space, so
    /// the resulting element bbox matches what a digital (non-OCR) page would produce
    /// for the same physical position — PDF points, origin bottom-left — not raw
    /// Tesseract raster pixels.
    #[cfg(feature = "pdf")]
    #[test]
    fn build_mixed_ocr_page_document_rescales_element_bbox_into_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut ocr_doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        // A word sitting near the top-left corner of a 1700x2200px raster.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 260.0,
        });
        ocr_doc.push_element(element);

        let mut result = crate::types::ExtractedDocument {
            content: "hello".to_string(),
            ocr_internal_document: Some(ocr_doc),
            ..Default::default()
        };

        // 1700x2200px raster of a 612x792pt (US Letter) page.
        let (page_doc, _paragraphs) = build_mixed_ocr_page_document(&mut result, 1, 1700, 2200, 612.0, 792.0)
            .expect("an OCR document with a text element must produce a page document");

        let hello_element = page_doc
            .elements
            .iter()
            .find(|element| element.text == "hello")
            .expect("the OCR paragraph must survive assembly");
        let bbox = hello_element.bbox.expect("assembled element must carry a bbox");
        // scale_x = scale_y = 0.36; top-left pixel (100, 200)-(300, 260) scales to
        // page height in points (792, not the 2200px raster height):
        //   bottom = 792 - 93.6 = 698.4, top = 792 - 72 = 720.0
        // Tolerance of 1e-3 accounts for the f32 arithmetic `pdf_block_bbox`
        // (`crate::pdf::structure::adapters`) performs on the flip, which this test
        // deliberately exercises end-to-end rather than re-deriving in f64.
        assert!((bbox.x0 - 36.0).abs() < 1e-3, "x0 = {}", bbox.x0);
        assert!((bbox.x1 - 108.0).abs() < 1e-3, "x1 = {}", bbox.x1);
        assert!((bbox.y0 - 698.4).abs() < 1e-3, "y0 (bottom) = {}", bbox.y0);
        assert!((bbox.y1 - 720.0).abs() < 1e-3, "y1 (top) = {}", bbox.y1);
        // GH#1423's defining symptom is a bbox that does not fit on the page at all,
        // so assert containment rather than a "near the top" heuristic. This is the
        // guard that actually bites: if the conversion regressed to emitting raster
        // pixels, y1 would be 2200 - 200 = 2000 and blow the 792pt bound, whereas a
        // "y1 is in the upper half" check would pass on that same broken output.
        assert!(
            bbox.x1 <= 612.0 && bbox.y1 <= 792.0,
            "every OCR bbox must fit within the 612x792pt page, got ({}, {})-({}, {})",
            bbox.x0,
            bbox.y0,
            bbox.x1,
            bbox.y1
        );
    }

    /// Unit-mismatch regression for `assemble_mixed_ocr_page_document`'s font-size
    /// resolution: `rescale_ocr_bboxes_to_page_points` (exercised by the previous test)
    /// rescales `element.bbox` into PDF points, but never touches `element.ocr_geometry`
    /// -- it stays raw OCR raster pixels, because `extraction::derive::OcrElement::geometry`
    /// documents that field as public raster-pixel-space API and rescaling it in place
    /// would corrupt that contract (and round away sub-pixel precision, since its point
    /// type is `(u32, u32)`). A `Quadrilateral`-geometry element -- sceptre and paddle's
    /// shape; Tesseract's `Rectangle` geometry never takes this branch -- must therefore
    /// have its quad-edge-height font-size proxy scaled by the page's *own*
    /// points-per-pixel ratio, not left in raw pixels.
    ///
    /// This fixture's raster/page pair (1700x2200px over 612x792pt, matching the previous
    /// test) gives a height-axis scale of 792 / 2200 = 0.36. The element's quad spans a
    /// 100px-tall band; the correctly-scaled font size is 100 * 0.36 = 36.0pt.
    ///
    /// Against unfixed code (`assemble_mixed_ocr_page_document` calling
    /// `ocr_doc_to_paragraphs(&doc, page_height, 1.0)` with one flat `1.0` scale for both
    /// the bbox and geometry fallback branches), this asserts `36.0` and fails: the actual
    /// value is `100.0`, the raw unscaled raster-pixel quad-edge height -- inflated by
    /// exactly the 1/0.36 ~= 2.78x this fixture's raster resolution implies (the ~2.08x
    /// figure quoted at 150 DPI is this same ratio at a different render resolution).
    #[cfg(feature = "pdf")]
    #[test]
    fn build_mixed_ocr_page_document_scales_quad_geometry_font_size_into_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut ocr_doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "SCEPTRE HEADING",
            0,
        );
        // Deliberately a different height from the quad below (150px vs 100px), so a
        // result of `150.0 * 0.36 = 54.0` (or unscaled `150.0`) would indicate the
        // bbox-height fallback fired instead of the quad-edge one -- it must not, since
        // `resolve_ocr_font_size_pt` always prefers `ocr_geometry` when present.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 500.0,
            y1: 350.0,
        });
        // A straight (unskewed) 100px-tall quad: `quad_edge_height_px` averages the two
        // side edges, both exactly 100px here, isolating this test from the
        // skew-robustness behavior `test_ocr_doc_uses_quad_edge_height_not_skewed_aabb_height_for_font_size`
        // (`pdf::structure::adapters`) already covers.
        element.ocr_geometry = Some(crate::types::OcrBoundingGeometry::Quadrilateral {
            points: [(100, 200), (500, 200), (500, 300), (100, 300)],
        });
        ocr_doc.push_element(element);

        let mut result = crate::types::ExtractedDocument {
            content: "SCEPTRE HEADING".to_string(),
            ocr_internal_document: Some(ocr_doc),
            ..Default::default()
        };

        let (_page_doc, paragraphs) = build_mixed_ocr_page_document(&mut result, 1, 1700, 2200, 612.0, 792.0)
            .expect("an OCR document with a quad-geometry text element must produce a page document");

        assert_eq!(paragraphs.len(), 1);
        assert!(
            (paragraphs[0].dominant_font_size - 36.0).abs() < 1e-3,
            "expected the quad-edge height rescaled into PDF points (100px * 0.36 = 36.0pt), got {}",
            paragraphs[0].dominant_font_size
        );
    }

    /// An already-assembled pipeline page document: one paragraph whose bbox is in the
    /// raster's *pixel* space with a bottom-left origin (`ocr_doc_to_paragraphs` has
    /// already flipped it using the raster height), plus one table carrying the raw
    /// top-left pixel rect that `push_table_element` copies onto its element.
    ///
    /// Raster is 1700x2200px; the paragraph sits 200-260px below the raster's top edge.
    #[cfg(feature = "pdf")]
    fn assembled_pipeline_page_document() -> crate::types::internal::InternalDocument {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        let mut paragraph = InternalElement::text(ElementKind::Paragraph, "hello", 0);
        paragraph.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 1940.0,
            x1: 300.0,
            y1: 2000.0,
        });
        doc.push_element(paragraph);

        let table_bbox = BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        };
        let mut table = ocr_table("| a | b |", 1);
        table.bounding_box = Some(table_bbox);
        let table_index = doc.push_table(table);
        let mut table_element = InternalElement::text(ElementKind::Table { table_index }, "", 0);
        table_element.bbox = Some(table_bbox);
        doc.push_element(table_element);

        doc
    }

    /// #529 (extends #1423) — the `vlm_fallback` / explicit-`pipeline` route builds its
    /// page document without going through `build_mixed_ocr_page_document`, so it never
    /// received the pixel -> point conversion at all and emitted raw raster pixels. The
    /// paragraph bbox is scaled only (it is already bottom-left), the table bbox and its
    /// element get the full scale-and-flip, and every box must fit on the page.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_emit_page_point_bboxes_when_ocr_runs_via_vlm_fallback_pipeline() {
        const PAGE_WIDTH_PT: f64 = 612.0;
        const PAGE_HEIGHT_PT: f64 = 792.0;

        // 1700x2200px raster of a 612x792pt (US Letter) page: scale_x = scale_y = 0.36.
        let page_doc = build_pipeline_ocr_page_document(
            Some(assembled_pipeline_page_document()),
            Vec::new(),
            Vec::new(),
            "hello",
            4,
            (1700, 2200),
            (PAGE_WIDTH_PT as f32, PAGE_HEIGHT_PT as f32),
        )
        .expect("a pipeline document must produce a page document");

        let paragraph = page_doc
            .elements
            .iter()
            .find(|element| element.text == "hello")
            .expect("the pipeline paragraph must survive");
        let bbox = paragraph.bbox.expect("paragraph must keep its bbox");
        assert_eq!(bbox.x0, 36.0);
        assert_eq!(bbox.y0, 698.4);
        assert_eq!(bbox.x1, 108.0);
        assert_eq!(bbox.y1, 720.0);
        // The defining symptom of #1423 is a box that cannot fit on the page: unconverted,
        // this paragraph reports y1 = 2000 against a 792pt page.
        assert!(
            bbox.x1 <= PAGE_WIDTH_PT && bbox.y1 <= PAGE_HEIGHT_PT,
            "paragraph bbox must fit within the page, got ({}, {})-({}, {})",
            bbox.x0,
            bbox.y0,
            bbox.x1,
            bbox.y1
        );

        let table_bbox = page_doc.tables[0]
            .bounding_box
            .expect("the table must keep its bounding box");
        assert_eq!(table_bbox.x0, 36.0);
        assert_eq!(table_bbox.y0, 648.0, "bottom = 792 - 400 * 0.36");
        assert_eq!(table_bbox.x1, 108.0);
        assert_eq!(table_bbox.y1, 720.0, "top = 792 - 200 * 0.36");
        assert!(
            table_bbox.x1 <= PAGE_WIDTH_PT && table_bbox.y1 <= PAGE_HEIGHT_PT,
            "table bbox must fit within the page, got ({}, {})-({}, {})",
            table_bbox.x0,
            table_bbox.y0,
            table_bbox.x1,
            table_bbox.y1
        );

        let table_element_bbox = page_doc
            .elements
            .iter()
            .find(|element| matches!(element.kind, crate::types::internal::ElementKind::Table { .. }))
            .and_then(|element| element.bbox)
            .expect("the table element must keep its bbox");
        assert_eq!(
            (
                table_element_bbox.x0,
                table_element_bbox.y0,
                table_element_bbox.x1,
                table_element_bbox.y1
            ),
            (36.0, 648.0, 108.0, 720.0),
            "the table element must report the same bottom-left point rect as its table"
        );
    }

    /// #529 — a pipeline stage that produced only a table (no structured document) must
    /// still get a page document whose table bbox is in page points, and a stage that
    /// produced nothing structured must still produce no document at all.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_convert_pipeline_table_bboxes_when_no_structured_document_is_returned() {
        use crate::types::extraction::BoundingBox;

        let mut table = ocr_table("| a | b |", 1);
        table.bounding_box = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });

        let page_doc = build_pipeline_ocr_page_document(
            None,
            vec![table],
            Vec::new(),
            "scanned prose",
            2,
            (1700, 2200),
            (612.0, 792.0),
        )
        .expect("a pipeline result with a table must produce a page document");

        assert_eq!(page_doc.tables.len(), 1);
        assert_eq!(page_doc.tables[0].page_number, 2, "table renumbered onto its page");
        let bbox = page_doc.tables[0].bounding_box.expect("table bbox must survive");
        assert_eq!((bbox.x0, bbox.y0, bbox.x1, bbox.y1), (36.0, 648.0, 108.0, 720.0));

        assert!(
            build_pipeline_ocr_page_document(
                None,
                Vec::new(),
                Vec::new(),
                "text only",
                2,
                (1700, 2200),
                (612.0, 792.0)
            )
            .is_none(),
            "a text-only pipeline result must keep the raw-text replacement path"
        );
    }

    /// A single-page PDF with a landscape 200x100pt MediaBox and the given `/Rotate`.
    ///
    /// Mirrors the fixture builder in `layout_runner`'s tests, which is where the
    /// rotation convention exercised below is established.
    #[cfg(feature = "pdf")]
    fn rotated_landscape_pdf(rotation: i64) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();
        let content_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));

        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
            "Resources" => dictionary! {},
            "Contents" => content_id,
        };
        page.set("Rotate", rotation);
        document.objects.insert(page_id, Object::Dictionary(page));

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );

        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    /// #530 — on a page with `/Rotate` 90 or 270 the OCR bbox conversion must still land
    /// inside the page. `pdf_oxide` renders such a page in *displayed* orientation (with
    /// width and height swapped relative to the MediaBox), but every OCR route rasterizes
    /// through `normalize_rendered_page_for_ocr`, which applies the inverse quarter turn
    /// and hands OCR a raster back in raw MediaBox orientation — the same convention
    /// `layout_runner::render_layout_chunk` encodes by using the raw MediaBox dimensions
    /// whenever `normalize_for_ocr` is set. `page_dimensions_pt` also returns the raw
    /// MediaBox, so the two agree and no axis swap belongs in the conversion.
    ///
    /// This test pins that agreement: it fails if the raster stops being MediaBox-oriented
    /// or if a swap is introduced into the conversion, either of which puts rotated-page
    /// boxes off the page.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_convert_ocr_bboxes_within_page_bounds_on_rotated_pages() {
        for rotation in [90, 270] {
            let bytes = rotated_landscape_pdf(rotation);
            let rendered = render_selected_pages_for_ocr(&bytes, &[0]).expect("rotated page must render for OCR");
            let (_, image) = rendered.first().expect("page 0 must be rendered");
            let (raster_width_px, raster_height_px) = (image.width(), image.height());

            let document = pdf_oxide::PdfDocument::from_bytes(bytes.clone()).expect("fixture PDF must open");
            let (page_width_pt, page_height_pt) = page_dimensions_pt(&document, 0);
            assert_eq!(
                (page_width_pt, page_height_pt),
                (200.0, 100.0),
                "/Rotate {rotation}: page_dimensions_pt reports the raw MediaBox"
            );
            assert!(
                raster_width_px > raster_height_px,
                "/Rotate {rotation}: the OCR raster must keep the MediaBox's landscape \
                 orientation, got {raster_width_px}x{raster_height_px}"
            );

            // A table box covering the whole raster must convert to exactly the whole page.
            let mut tables = [ocr_table("| a | b |", 1)];
            tables[0].bounding_box = Some(crate::types::extraction::BoundingBox {
                x0: 0.0,
                y0: 0.0,
                x1: f64::from(raster_width_px),
                y1: f64::from(raster_height_px),
            });
            rescale_ocr_bboxes_to_page_points(
                None,
                &mut tables,
                raster_width_px,
                raster_height_px,
                page_width_pt,
                page_height_pt,
            );

            let bbox = tables[0].bounding_box.expect("table bbox must survive rescale");
            assert_eq!(
                (bbox.x0, bbox.y0, bbox.x1, bbox.y1),
                (0.0, 0.0, 200.0, 100.0),
                "/Rotate {rotation}: a full-raster box must map onto the full page"
            );
            assert!(
                bbox.x1 <= f64::from(page_width_pt) && bbox.y1 <= f64::from(page_height_pt),
                "/Rotate {rotation}: converted bbox must fit within the page"
            );
        }
    }

    /// Closes the gap left by 972d2269f7: that commit threaded a `page_rotation_degrees`
    /// hint into the full-page and per-page/mixed OCR routes via
    /// `ocr_config_with_page_rotation_hint`, but `extract_with_ocr`'s images-driven branch
    /// (the one the layout-detection route feeds pre-rendered pages through, see
    /// `crates/xberg/src/extractors/pdf/mod.rs`) never computed a rotation at all: its only
    /// rotation source, `lazy_pdf_render_state`, is only populated when `images.is_none()`
    /// (see the `if !use_document_processing && images.is_none()` guard above), so every
    /// lookup fell through to `unwrap_or(0)`.
    ///
    /// Fails on unfixed code: `ocr_config_with_page_rotation_hint` is documented as a no-op
    /// for a rotation of 0, so an unfixed call leaves `OcrConfig.backend_options` at `None`
    /// instead of carrying `{"page_rotation_degrees": 270}` for a `/Rotate 270` page.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_thread_page_rotation_hint_through_the_images_driven_ocr_route() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct RotationCapturingBackend {
            captured_backend_options: Mutex<Vec<Option<serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RotationCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], config: &OcrConfig) -> crate::Result<ExtractedDocument> {
                self.captured_backend_options
                    .lock()
                    .unwrap()
                    .push(config.backend_options.clone());
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RotationCapturingBackend {
            fn name(&self) -> &str {
                "rotation-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(RotationCapturingBackend {
            captured_backend_options: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Same fixture convention as `should_convert_ocr_bboxes_within_page_bounds_on_rotated_pages`:
        // a landscape MediaBox with `/Rotate 270`, mirroring the ordinance-scan case this
        // session's fix (972d2269f7) targeted.
        let content = rotated_landscape_pdf(270);
        let rendered = render_selected_pages_for_ocr(&content, &[0]).expect("rotated fixture page must render for OCR");
        let images: Vec<image::DynamicImage> = rendered.into_iter().map(|(_, image)| image).collect();

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "rotation-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // `content` is passed alongside `images`, mirroring the real layout-detection call
        // site (`crates/xberg/src/extractors/pdf/mod.rs`), which always has the original PDF
        // bytes available even when it hands in pre-rendered images.
        let result = extract_with_ocr(
            Some(&content),
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("rotation-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_backend_options.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        let rotation_hint = captured[0]
            .as_ref()
            .and_then(|opts| opts.get("page_rotation_degrees"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            rotation_hint,
            Some(270),
            "the images-driven OCR route (used by layout detection) must carry the page's \
             /Rotate value into backend_options, exactly as the full-page and per-page routes \
             already do (972d2269f7); got backend_options = {:?}",
            captured[0]
        );
    }

    /// #643 — a backend that declares `PageOrientationHandling::RequiresUpright` must receive
    /// an upright raster on a rotated page, not the same MediaBox-oriented (sideways) raster
    /// every other backend gets. `normalize_rendered_page_for_ocr` deliberately hands every
    /// backend that raw-orientation raster (#530); Tesseract self-corrects it and PaddleOCR
    /// recognises the rotated text correctly (only block order needs fixing), but a
    /// `RequiresUpright` backend (sceptre in production) emits character garbage on it instead
    /// — confirmed this session by feeding the same page through an upright render.
    ///
    /// Fails on unfixed code: without `upright_raster_for_backend` wired into the OCR call, the
    /// backend receives the same landscape MediaBox raster as every other backend (width >
    /// height, per the #530 invariant on this fixture) instead of the portrait upright raster
    /// asserted below.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_send_an_upright_raster_to_a_requires_upright_backend_on_a_rotated_page() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct RequiresUprightCapturingBackend {
            captured_dimensions: Mutex<Vec<(u32, u32)>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RequiresUprightCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::RequiresUpright
            }
            async fn process_image(&self, image_bytes: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let decoded = image::load_from_memory(image_bytes).expect("captured raster bytes must decode");
                self.captured_dimensions
                    .lock()
                    .unwrap()
                    .push((decoded.width(), decoded.height()));
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RequiresUprightCapturingBackend {
            fn name(&self) -> &str {
                "requires-upright-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(RequiresUprightCapturingBackend {
            captured_dimensions: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Same fixture convention as the two tests above: a landscape MediaBox with
        // `/Rotate 270`. The raw MediaBox raster (what every backend got before this fix) keeps
        // that landscape shape (width > height, #530); the upright raster this backend needs is
        // the display orientation instead, portrait (width < height).
        let content = rotated_landscape_pdf(270);

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "requires-upright-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&content),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("requires-upright-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_dimensions.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        let (width, height) = captured[0];
        assert!(
            width < height,
            "a RequiresUpright backend must receive an upright (portrait) raster on a \
             /Rotate 270 page, not the raw landscape MediaBox raster every other backend gets; \
             got {width}x{height}"
        );
    }

    /// #643 pin — a backend that does NOT declare `RequiresUpright` (e.g. Tesseract's
    /// `SelfCorrecting`) must keep receiving exactly the raster this route has always sent it:
    /// the raw MediaBox-oriented raster `render_full_pdf_ocr_batch`/
    /// `normalize_rendered_page_for_ocr` produce (#530), byte-for-byte. Tesseract's output is
    /// the control for every OCR-rotation measurement this session; a future change that starts
    /// rotating its input for the wrong backend would invalidate that measurement silently, and
    /// this test exists so that cannot happen without failing here first.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_leave_a_self_correcting_backends_raster_byte_for_byte_unchanged() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct SelfCorrectingCapturingBackend {
            captured_bytes: Mutex<Vec<Vec<u8>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for SelfCorrectingCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::SelfCorrecting
            }
            async fn process_image(&self, image_bytes: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                self.captured_bytes.lock().unwrap().push(image_bytes.to_vec());
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for SelfCorrectingCapturingBackend {
            fn name(&self) -> &str {
                "self-correcting-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(SelfCorrectingCapturingBackend {
            captured_bytes: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        let content = rotated_landscape_pdf(270);

        // Independently reproduce the exact raster this route sends every backend, via the
        // same private helpers `extract_with_ocr` itself calls, so the comparison below is
        // byte-for-byte against production behaviour rather than a re-derived approximation.
        let (expected_doc, _page_count, expected_rotations) =
            open_pdf_for_full_ocr(&content).expect("fixture PDF must open for OCR rendering");
        let expected_encoded =
            render_full_pdf_ocr_batch(&expected_doc, &expected_rotations, 0..1).expect("fixture page must render");
        let (_, expected_bytes, _, _) = expected_encoded.into_iter().next().expect("page 0 must render");

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "self-correcting-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&content),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("self-correcting-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_bytes.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        assert_eq!(
            captured[0], *expected_bytes,
            "a SelfCorrecting backend's raster must stay byte-for-byte identical to the raw \
             MediaBox raster this route has always sent it; this pins that a future orientation \
             fix cannot silently start rotating it"
        );
    }

    /// Two-page PDF: page 1 an ordinary unrotated portrait page, page 2 a landscape MediaBox
    /// with the given `/Rotate`. Lets a test target `ocr_page_numbers = &[2]` and prove the
    /// rotation applied is page 2's own value, not page 0's/page 1's -- `rotated_landscape_pdf`
    /// above only builds a single page, which cannot distinguish "the correct page's rotation"
    /// from "whatever happens to be at index 0".
    #[cfg(feature = "pdf")]
    fn two_page_pdf_second_page_rotated(rotation: i64) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page1_id = document.new_object_id();
        let page2_id = document.new_object_id();
        let content1_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let content2_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));

        let page1 = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 200.into()],
            "Resources" => dictionary! {},
            "Contents" => content1_id,
        };
        document.objects.insert(page1_id, Object::Dictionary(page1));

        let mut page2 = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
            "Resources" => dictionary! {},
            "Contents" => content2_id,
        };
        page2.set("Rotate", rotation);
        document.objects.insert(page2_id, Object::Dictionary(page2));

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page1_id.into(), page2_id.into()],
                "Count" => 2,
            }),
        );

        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    /// #651 — closes the gap in `extract_mixed_ocr_native`'s multi-stage pipeline route: its
    /// two `run_ocr_pipeline` calls passed `content: None` and, before this fix, had no
    /// parameter at all carrying the page's own `/Rotate` value, so neither the
    /// `ocr_config_with_page_rotation_hint` block-order hint nor the
    /// `upright_raster_for_backend` upright-raster correction ever reached a stage backend on
    /// this route — unlike the sibling single-backend route (972d2269f7 / #643), which already
    /// applies both.
    ///
    /// Targets page 2 of a 2-page document (not page 1) so the test cannot pass by accident:
    /// a fix that always resolved page 0's/page 1's rotation instead of the actually-OCR'd
    /// page's (e.g. from a naive index-based `content` lookup misaligned to a single detached
    /// image, which is exactly what a `content: Some(content)` fix without the explicit
    /// per-page override would have done) would still leave this backend on the wrong raster.
    ///
    /// Fails on unfixed code: without the rotation threaded from `extract_mixed_ocr_native`
    /// through `run_ocr_pipeline` to the stage's OCR call, the `RequiresUpright` backend
    /// receives the raw landscape MediaBox raster (width > height, per the #530 invariant) and
    /// no `page_rotation_degrees` in `backend_options`, so both assertions below fail.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_pipeline_route_threads_page_rotation_to_a_requires_upright_backend() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::{Arc, Mutex};

        struct RequiresUprightCapturingPipelineBackend {
            captured_dimensions: Mutex<Vec<(u32, u32)>>,
            captured_backend_options: Mutex<Vec<Option<serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RequiresUprightCapturingPipelineBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::RequiresUpright
            }
            async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let decoded = image::load_from_memory(image_bytes).expect("captured raster bytes must decode");
                self.captured_dimensions
                    .lock()
                    .unwrap()
                    .push((decoded.width(), decoded.height()));
                self.captured_backend_options
                    .lock()
                    .unwrap()
                    .push(config.backend_options.clone());
                Ok(ExtractedDocument {
                    content: "page two ocr text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RequiresUprightCapturingPipelineBackend {
            fn name(&self) -> &str {
                "pipeline-requires-upright-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(RequiresUprightCapturingPipelineBackend {
            captured_dimensions: Mutex::new(Vec::new()),
            captured_backend_options: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Page 2 is landscape (200x100) with `/Rotate 270`; page 1 is an ordinary portrait
        // page and is never OCR'd (`ocr_page_numbers = &[2]` below), so a fix that resolved
        // page 1's (nonexistent) rotation instead of page 2's own would not accidentally
        // satisfy this test.
        let pdf = two_page_pdf_second_page_rotated(270);

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: "pipeline-requires-upright-capturing-mock".to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            // Accept the first (only) stage unconditionally: this test exercises rotation
            // threading, not the quality-based selection policy.
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline),
                ..Default::default()
            }),
            ..Default::default()
        };

        // `accepted_ocr_page_replacements` (which the returned map at index 1 reflects) only
        // accepts an OCR'd page whose number has a matching, non-overlapping native-text
        // boundary, so both pages need real boundaries even though only page 2 is OCR'd.
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("pipeline-requires-upright-capturing-mock").unwrap();

        assert!(
            result.1.contains_key(&2),
            "page 2's OCR text must have been accepted as a replacement: {:?}",
            result.1
        );

        let captured_dims = backend.captured_dimensions.lock().unwrap();
        assert_eq!(captured_dims.len(), 1, "backend should have been called exactly once");
        let (width, height) = captured_dims[0];
        assert!(
            width < height,
            "a RequiresUpright backend driven through the multi-stage pipeline route must \
             receive an upright (portrait) raster for a /Rotate 270 page, not the raw \
             landscape MediaBox raster every backend used to get; got {width}x{height}"
        );

        let captured_options = backend.captured_backend_options.lock().unwrap();
        let rotation_hint = captured_options[0]
            .as_ref()
            .and_then(|opts| opts.get("page_rotation_degrees"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            rotation_hint,
            Some(270),
            "the multi-stage pipeline route must also carry the page's /Rotate value into \
             backend_options, exactly as the single-backend route already does (972d2269f7); \
             got backend_options = {:?}",
            captured_options[0]
        );
    }

    /// Build a single-line OCR "block" element carrying an hOCR `x_fsize` (points)
    /// attribute, mirroring what `ocr::hocr_parser` attaches for tesseract output.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    fn ocr_font_block(text: &str, font_size_pt: &str, y0: f64, y1: f64) -> crate::types::internal::InternalElement {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            text,
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0,
            x1: 500.0,
            y1,
        });
        elem.attributes = Some(
            [("x_fsize".to_string(), font_size_pt.to_string())]
                .into_iter()
                .collect(),
        );
        elem
    }

    /// Two-page `Vec<Vec<PdfParagraph>>` with one large-font heading block, two
    /// ordered-list-marker blocks, and enough body prose (all at the same font size)
    /// to clear `MIN_BLOCKS_FOR_FONT_HEADING` and establish a reliable body-font
    /// baseline for the k-means heading heuristic.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    fn heading_and_list_ocr_pages() -> Vec<Vec<crate::pdf::structure::types::PdfParagraph>> {
        let mut page1 = crate::types::internal::InternalDocument::new("test");
        page1.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
        page1.push_element(ocr_font_block(
            "This document summarizes the annual results for the reporting period in detail.",
            "11",
            60.0,
            90.0,
        ));
        page1.push_element(ocr_font_block(
            "1. First item in the numbered list of findings",
            "11",
            100.0,
            130.0,
        ));
        page1.push_element(ocr_font_block(
            "2. Second item continuing the numbered list of findings",
            "11",
            140.0,
            170.0,
        ));
        page1.push_element(ocr_font_block(
            "Additional narrative text follows describing the broader context for readers.",
            "11",
            180.0,
            210.0,
        ));

        let mut page2 = crate::types::internal::InternalDocument::new("test");
        page2.push_element(ocr_font_block(
            "Further discussion continues here with more explanatory prose for the reader.",
            "11",
            10.0,
            40.0,
        ));
        page2.push_element(ocr_font_block(
            "Another paragraph of body text appears on the second page of the document.",
            "11",
            50.0,
            80.0,
        ));

        vec![
            crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
                &page1,
                1000,
                crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
            ),
            crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
                &page2,
                1000,
                crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
            ),
        ]
    }

    /// Pins the WP-A fix: today (before wiring OCR paragraphs through
    /// `extract_document_structure_from_segments`) `heuristically_restructured_ocr_pages`
    /// does not exist and the OCR route never promotes anything, so a scanned-PDF
    /// Markdown extraction shows zero headings and zero list items regardless of how
    /// font sizes vary across the page (`dominant_font_size` was hardcoded to `12.0`
    /// for every OCR paragraph). This must fail against that code: it asserts both a
    /// `Heading` and a `ListItem` element are present.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_promotes_headings_and_lists_for_markdown() {
        let pages = heading_and_list_ocr_pages();
        let mut config = ExtractionConfig::default();
        config.output_format = crate::core::config::OutputFormat::Markdown;

        let doc = heuristically_restructured_ocr_pages(&pages, &[], &config)
            .expect("font-size variation and list markers should produce a restructured document");

        assert!(
            doc.elements
                .iter()
                .any(|element| matches!(element.kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected at least one Heading element, got kinds: {:?}",
            doc.elements.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
        assert!(
            doc.elements
                .iter()
                .any(|element| matches!(element.kind, crate::types::internal::ElementKind::ListItem { .. })),
            "expected at least one ListItem element, got kinds: {:?}",
            doc.elements.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
    }

    /// Pins the `Plain` gate: the exact same input that produces headings and list
    /// items under `Markdown` (previous test) must be left untouched under `Plain` --
    /// `heuristically_restructured_ocr_pages` must return `None` so the caller keeps
    /// its pre-existing, unstructured assembly and Plain-format output stays
    /// byte-identical to before WP-A. A version of the fix that forgot the
    /// `output_format` gate (ran the heuristic unconditionally) would fail this.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_is_noop_for_plain_output() {
        let pages = heading_and_list_ocr_pages();
        let config = ExtractionConfig::default();
        assert_eq!(config.output_format, crate::core::config::OutputFormat::Plain);

        assert!(heuristically_restructured_ocr_pages(&pages, &[], &config).is_none());
    }

    /// Pins the ML-layout precedence: when `ocr_doc_to_layout_paragraphs` (or
    /// `promote_anchored_ordered_list_sequences`) has already classified a paragraph
    /// as a heading or list item, the segment heuristic must not run at all --
    /// re-deriving structure from bare segments would discard that classification
    /// instead of adding to it (the ML-layout OCR route already measures correct
    /// headings on the reference fixture; this test only pins the *skip*, not that
    /// route's own output).
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_skips_when_already_layout_classified() {
        let mut pages = heading_and_list_ocr_pages();
        pages[0][0].heading_level = Some(1);
        let mut config = ExtractionConfig::default();
        config.output_format = crate::core::config::OutputFormat::Markdown;

        assert!(heuristically_restructured_ocr_pages(&pages, &[], &config).is_none());
    }

    /// Closes the gap this session's fix targets: before it,
    /// `heuristically_restructured_ocr_pages` had exactly one call site, inside
    /// `extract_with_ocr_for_page` -- but `extract_mixed_ocr_native`'s single-backend
    /// route (`--ocr-scanned-pages` / `--force-ocr-pages` with no pipeline configured)
    /// builds its per-page document via `build_mixed_ocr_page_document` ->
    /// `assemble_mixed_ocr_page_document`, which goes straight from bare paragraphs to
    /// `assemble_internal_document` and never calls the heuristic at all. So a scanned
    /// PDF extracted through this route emitted zero headings under Markdown output no
    /// matter how much its font sizes varied.
    ///
    /// The mock backend returns the *same* heading+body content for every page
    /// (`ocr_font_block`, reused from `heading_and_list_ocr_pages`'s per-page fixture)
    /// deliberately: this route's per-page OCR calls run concurrently through a
    /// `JoinSet`, so nothing here may depend on call order or backend-visible page
    /// identity.
    ///
    /// Fails on unfixed code: `structured_ocr_pages` contains only `Paragraph`
    /// elements (from `assemble_internal_document` over unclassified paragraphs), no
    /// `Heading`.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_single_backend_route_promotes_document_global_headings() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct HeadingMockBackend;

        fn heading_body_document() -> crate::types::internal::InternalDocument {
            let mut doc = crate::types::internal::InternalDocument::new("test");
            doc.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
            doc.push_element(ocr_font_block(
                "This document summarizes the annual results for the reporting period in detail.",
                "11",
                60.0,
                90.0,
            ));
            doc.push_element(ocr_font_block(
                "1. First item in the numbered list of findings",
                "11",
                100.0,
                130.0,
            ));
            doc.push_element(ocr_font_block(
                "2. Second item continuing the numbered list of findings",
                "11",
                140.0,
                170.0,
            ));
            doc.push_element(ocr_font_block(
                "Additional narrative text follows describing the broader context for readers.",
                "11",
                180.0,
                210.0,
            ));
            doc
        }

        #[async_trait::async_trait]
        impl OcrBackend for HeadingMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "ANNUAL REPORT OVERVIEW body text with enough words to avoid the \
                              recognition-noise gate"
                        .to_string(),
                    ocr_internal_document: Some(heading_body_document()),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for HeadingMockBackend {
            fn name(&self) -> &str {
                "doc-global-heuristic-mixed-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(HeadingMockBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "doc-global-heuristic-mixed-mock".to_string(),
                ..Default::default()
            }),
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[1, 2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("doc-global-heuristic-mixed-mock").unwrap();

        let structured_pages = result.2;
        assert!(
            !structured_pages.is_empty(),
            "expected at least one structured OCR page"
        );
        let all_kinds: Vec<_> = structured_pages
            .values()
            .flat_map(|doc| doc.elements.iter().map(|e| e.kind))
            .collect();
        assert!(
            all_kinds
                .iter()
                .any(|kind| matches!(kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected the document-global heading heuristic to promote a Heading element on \
             the mixed OCR single-backend route; got element kinds: {all_kinds:?}"
        );
    }

    /// Same defect, pipeline route: `run_ocr_pipeline_for_page` drives each OCR'd page
    /// through `extract_with_ocr_for_page` with exactly one detached image per call
    /// (`std::slice::from_ref`), so even though that function's heuristic call site
    /// technically runs, it only ever sees one page at a time -- too few blocks/pages
    /// for `build_heading_map`'s clustering to promote anything reliably, and (before
    /// `skip_document_global_heuristic` existed) any classification it *did* manage
    /// would mark the page's paragraphs "already structured" and block this test's
    /// document-wide pass from running at all. `points_per_pixel_override` and
    /// `skip_document_global_heuristic` together are what let this route feed the
    /// same document-global pass every page's bare paragraphs at once, exactly like
    /// the single-backend route above.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_pipeline_route_promotes_document_global_headings() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct HeadingMockPipelineBackend;

        fn heading_body_document() -> crate::types::internal::InternalDocument {
            let mut doc = crate::types::internal::InternalDocument::new("test");
            doc.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
            doc.push_element(ocr_font_block(
                "This document summarizes the annual results for the reporting period in detail.",
                "11",
                60.0,
                90.0,
            ));
            doc.push_element(ocr_font_block(
                "1. First item in the numbered list of findings",
                "11",
                100.0,
                130.0,
            ));
            doc.push_element(ocr_font_block(
                "2. Second item continuing the numbered list of findings",
                "11",
                140.0,
                170.0,
            ));
            doc.push_element(ocr_font_block(
                "Additional narrative text follows describing the broader context for readers.",
                "11",
                180.0,
                210.0,
            ));
            doc
        }

        #[async_trait::async_trait]
        impl OcrBackend for HeadingMockPipelineBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "ANNUAL REPORT OVERVIEW body text with enough words to avoid the \
                              recognition-noise gate"
                        .to_string(),
                    ocr_internal_document: Some(heading_body_document()),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for HeadingMockPipelineBackend {
            fn name(&self) -> &str {
                "doc-global-heuristic-pipeline-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(HeadingMockPipelineBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "doc-global-heuristic-pipeline-mock".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    // Accept the first (only) stage unconditionally: this test exercises
                    // structure promotion, not the quality-based selection policy.
                    quality_thresholds: OcrQualityThresholds {
                        pipeline_min_quality: 0.0,
                        ..Default::default()
                    },
                }),
                ..Default::default()
            }),
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[1, 2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("doc-global-heuristic-pipeline-mock").unwrap();

        let structured_pages = result.2;
        assert!(
            !structured_pages.is_empty(),
            "expected at least one structured OCR page"
        );
        let all_kinds: Vec<_> = structured_pages
            .values()
            .flat_map(|doc| doc.elements.iter().map(|e| e.kind))
            .collect();
        assert!(
            all_kinds
                .iter()
                .any(|kind| matches!(kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected the document-global heading heuristic to promote a Heading element on \
             the mixed OCR pipeline route; got element kinds: {all_kinds:?}"
        );
    }

    /// Text an OCR backend returns when handed the recovered image XObject bytes.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    const XOBJECT_RECOVERED_TEXT: &str = "RECOVERED FROM EMBEDDED IMAGE XOBJECT";

    /// The reporter's error in #1444, verbatim.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    const VLM_NO_CONTENT_ERROR: &str = "VLM OCR returned no content";

    /// Single page carrying exactly one embedded DCT/JPEG image XObject; already relied on
    /// by `test_page_ocr_fallback_image_bytes_recovers_real_image` in
    /// `crate::pdf::oxide::images`.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn single_xobject_fixture_bytes() -> Vec<u8> {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(pdf_path.exists(), "missing fixture: {}", pdf_path.display());
        std::fs::read(&pdf_path).expect("failed to read test PDF fixture")
    }

    /// Whether these bytes are the recovered image XObject (an embedded JPEG) rather than
    /// the whole-page raster, which this module always encodes as PNG.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn is_embedded_jpeg(data: &[u8]) -> bool {
        data.starts_with(&[0xFF, 0xD8])
    }

    /// #1444, dominant hole: a per-page backend failure propagated straight out of
    /// `extract_with_ocr` with `?`, aborting the whole document *before* the blank-page
    /// image-XObject fallback further down the same loop could ever run. That is exactly the
    /// reporter's symptom -- a scanned PDF dying with "OCR error: VLM OCR returned no
    /// content" while the recovery that would have saved the page sat unreachable.
    ///
    /// The page must now degrade into that fallback, and the failure must still be visible.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_page_from_image_xobjects_when_the_backend_errors_on_the_page_raster() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "page-raster-error-fallback-test-backend";

        struct FailOnPageRasterBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FailOnPageRasterBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, data: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                if is_embedded_jpeg(data) {
                    Ok(ExtractedDocument {
                        content: XOBJECT_RECOVERED_TEXT.to_string(),
                        ..Default::default()
                    })
                } else {
                    Err(crate::XbergError::Plugin {
                        message: VLM_NO_CONTENT_ERROR.to_string(),
                        plugin_name: "ocr".to_string(),
                    })
                }
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FailOnPageRasterBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FailOnPageRasterBackend)).unwrap();

        let pdf_bytes = single_xobject_fixture_bytes();
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, _, doc, _, _, _, _) =
            result.expect("a per-page backend failure must degrade to the XObject fallback, not abort the document");
        assert_eq!(
            text, XOBJECT_RECOVERED_TEXT,
            "the failed page's text must come from its embedded image XObject"
        );

        let warnings = doc
            .expect("the fallback and failure warnings must produce an internal document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR was retried on the embedded image bytes")),
            "the XObject fallback must be reported; got: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR of page 1 failed") && w.message.contains(VLM_NO_CONTENT_ERROR)),
            "the backend failure must survive as a warning rather than vanish; got: {warnings:?}"
        );
    }

    /// #1444, holes 2 and 4 together. Two independent reasons the fallback could not fire
    /// for a layout-detected scanned page:
    ///
    /// 1. pre-rendered `images` left `lazy_pdf_render_state` unopened, so the fallback had
    ///    no document to read XObjects from and was skipped outright;
    /// 2. a chatty backend that *describes* the blank page ("The image is entirely blank.")
    ///    clears `is_page_text_blank`'s 3-non-whitespace-character floor, so the page did
    ///    not even look blank.
    ///
    /// Here the supplied page raster is pure white -- there is no ink on it at all -- so the
    /// backend's answer is a description, not a transcription, and the page's embedded image
    /// XObject is what actually carries the text.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_blank_inked_page_when_backend_describes_blankness_over_prerendered_images() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "chatty-blank-description-test-backend";
        const BLANK_DESCRIPTION: &str = "The image is entirely blank.";

        struct DescribesBlankPageBackend;

        #[async_trait::async_trait]
        impl OcrBackend for DescribesBlankPageBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, data: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let content = if is_embedded_jpeg(data) {
                    XOBJECT_RECOVERED_TEXT
                } else {
                    BLANK_DESCRIPTION
                };
                Ok(ExtractedDocument {
                    content: content.to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for DescribesBlankPageBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(DescribesBlankPageBackend)).unwrap();

        // Stands in for what layout detection hands the OCR loop: an already-rendered page
        // image. All white, i.e. exactly what pdf_oxide substitutes for a page whose image
        // XObjects it could not draw.
        let blank_page = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 260, image::Rgb([255; 3])));
        let pdf_bytes = single_xobject_fixture_bytes();
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            Some(std::slice::from_ref(&blank_page)),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, _, _, _, _, _, _) = result.expect("OCR must succeed");
        assert_eq!(
            text, XOBJECT_RECOVERED_TEXT,
            "a description of a blank raster must not be accepted as that page's transcription"
        );
    }

    /// #1444, hole 3: when every pipeline stage errors outright, no stage ever reached its
    /// own per-page fallback, and `run_ocr_pipeline` gave up with "All OCR pipeline backends
    /// failed" having never looked at the pages' embedded images.
    ///
    /// The backend here fails its first two calls -- the whole-page raster, then that page's
    /// one image XObject inside the stage -- so the stage genuinely produces nothing and the
    /// pipeline exhausts its only stage. The third call is the pipeline's own last-resort
    /// recovery, which must run and must succeed.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_from_image_xobjects_when_every_pipeline_stage_fails() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const BACKEND_NAME: &str = "pipeline-all-stages-fail-test-backend";

        struct FailsFirstTwoCallsBackend {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for FailsFirstTwoCallsBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call <= 2 {
                    return Err(crate::XbergError::Plugin {
                        message: VLM_NO_CONTENT_ERROR.to_string(),
                        plugin_name: "ocr".to_string(),
                    });
                }
                Ok(ExtractedDocument {
                    content: XOBJECT_RECOVERED_TEXT.to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FailsFirstTwoCallsBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        crate::plugins::register_ocr_backend(Arc::new(FailsFirstTwoCallsBackend { calls: calls.clone() })).unwrap();

        let pdf_bytes = single_xobject_fixture_bytes();
        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: Default::default(),
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _) =
            result.expect("the pipeline must try the pages' embedded images before reporting total failure");
        assert_eq!(text, XOBJECT_RECOVERED_TEXT);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "expected one page-raster call, one in-stage fallback call, and one pipeline-level recovery call"
        );

        let warnings = doc
            .expect("recovery warnings must produce a document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR was retried on the embedded image bytes")),
            "the pipeline-level recovery must be reported; got: {warnings:?}"
        );
    }

    /// #1444: degrading per-page failures to warnings must not turn a wholesale OCR failure
    /// into a silently empty document. This PDF has no content stream and no image XObjects,
    /// so there is nothing to recover and the error must still be reported -- but as one
    /// aggregate failure naming the page count, not as the raw first backend error.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_error_when_every_page_fails_and_nothing_can_be_recovered() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "always-failing-unrecoverable-test-backend";

        struct AlwaysFailingBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysFailingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Err(crate::XbergError::Plugin {
                    message: VLM_NO_CONTENT_ERROR.to_string(),
                    plugin_name: "ocr".to_string(),
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysFailingBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysFailingBackend)).unwrap();

        let pdf_bytes = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let error = result
            .expect_err("a document with no recoverable page must still fail")
            .to_string();
        assert!(
            error.contains("OCR failed on all 1 page(s)") && error.contains(VLM_NO_CONTENT_ERROR),
            "the aggregate failure must name the page count and the root cause; got: {error}"
        );
    }

    /// Supporting unit test for the ink sentinel (not a regression test -- the function is
    /// new in #1444). Pins the two contracts the guard depends on: an all-white raster reads
    /// as blank, and a raster carrying even a small mark does not.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn page_raster_is_blank_distinguishes_an_empty_raster_from_an_inked_one() {
        fn encode_png(img: &image::RgbImage) -> Vec<u8> {
            use image::ImageEncoder;
            let mut out = std::io::Cursor::new(Vec::new());
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(img, img.width(), img.height(), image::ColorType::Rgb8.into())
                .expect("PNG encode must succeed");
            out.into_inner()
        }

        let white = image::RgbImage::from_pixel(400, 500, image::Rgb([255; 3]));
        assert!(page_raster_is_blank(&encode_png(&white)));

        let mut inked = white.clone();
        for y in 100..140 {
            for x in 100..140 {
                inked.put_pixel(x, y, image::Rgb([0; 3]));
            }
        }
        assert!(!page_raster_is_blank(&encode_png(&inked)));

        // A short answer over an inked raster is a real (if terse) transcription and must
        // not be escalated; over a blank one it is a description of blankness.
        assert!(!page_needs_xobject_fallback("Invoice 4471", &encode_png(&inked)));
        assert!(page_needs_xobject_fallback(
            "The image is entirely blank.",
            &encode_png(&white)
        ));
    }
}

/// Coverage for the OCR recognition-noise veto.
///
/// The asymmetry is the whole design: a false positive deletes a page of real text, a false
/// negative only leaves noise in place. So the negative cases here — legitimate pages that
/// must survive — carry more weight than the positive ones.
#[cfg(all(test, any(feature = "ocr", feature = "ocr-pipeline")))]
mod recognition_noise_tests {
    use super::{NativeTextStats, is_ocr_recognition_noise, ocr_output_stats, repair_ocr_list_markers};
    use crate::core::config::OcrQualityThresholds;

    /// Verbatim from page 4 of a recorded municipal ordinance: Tesseract run over a scanned
    /// surveyor's plat. Every "word" here is an artifact of line art, not text.
    const PLAT_DRAWING_NOISE: &str = "\
LAKE POINTE SECTION 5 PLAT NU. 20060126 F.B.C.P.R. \
1 |: LAKE POINTE Zt } = ti | SECTION 4 - / ae | | | PLal NG. 200601237 A 5 oe \
: { L -W.5..R- —— 2 oe | ‘a. * MOI ARES a es \
MAM RAM SAL, Eid wat au TH.8) FLAT. ” <*> suum he tet cu? Oe imer \
AT ace im Cum BOCES SIOT TT. ie 4S ayi.0- 2 ub vee gman Suita \
‘1mC bo int (aa so Givicengo. Cunt A. cumin Lipa THIS mat Saakt Of MLSINICIED \
ot ee Steric im wt Pum ic or * oud OF o nue ow paapnace & ft LibuR DIMtChY";

    /// Verbatim from page 1 of the same document — ordinary legal prose.
    const ORDINANCE_PROSE: &str = "\
WHEREAS, the current property owner has requested that approximately 0.7906 acres of \
land located within the City of Sugar Land (the \"City\"), at the Southeast corner of Lake \
Pointe Parkway and Creek Bend Drive, be rezoned from Business Office (B-O) District to \
Planned Development (PD) District Final Development Plan; and WHEREAS, the City Planning \
and Zoning Commission forwarded its final report to the City Council, recommending \
approval of the rezoning request; and";

    #[test]
    fn should_reject_ocr_of_a_scanned_drawing() {
        let thresholds = OcrQualityThresholds::default();
        let stats = NativeTextStats::compute(PLAT_DRAWING_NOISE, &thresholds);

        assert!(
            stats.fragmented_word_ratio >= 0.35,
            "fixture is not representative: short-word ratio is {:.3}, expected >= 0.35",
            stats.fragmented_word_ratio
        );
        assert!(is_ocr_recognition_noise(PLAT_DRAWING_NOISE, &thresholds));
    }

    #[test]
    fn should_keep_ordinary_prose() {
        let thresholds = OcrQualityThresholds::default();
        assert!(!is_ocr_recognition_noise(ORDINANCE_PROSE, &thresholds));
    }

    #[test]
    fn should_decline_to_judge_a_page_with_too_few_words() {
        // The ratio is not meaningful on a handful of tokens, so the veto must abstain
        // rather than delete. This fixture is a real excerpt from the same plat page and
        // clears the ratio easily — only the word-count guard keeps it.
        let sliver = "1 |: Zt } = ti / ae A 5 oe : { L 2 oe";
        let thresholds = OcrQualityThresholds::default();
        let stats = NativeTextStats::compute(sliver, &thresholds);

        assert!(
            stats.fragmented_word_ratio >= thresholds.max_ocr_output_fragmented_word_ratio,
            "fixture must exceed the ratio ({:.3}), or the word-count guard is not what is \
             being tested",
            stats.fragmented_word_ratio
        );
        assert!(stats.word_count < thresholds.min_words_for_ocr_output_check);
        assert!(!is_ocr_recognition_noise(sliver, &thresholds));
    }

    #[test]
    fn should_keep_a_signature_block() {
        // Legitimately short-word-heavy prose that must survive on the ratio alone, with no
        // help from the word-count guard.
        let signature_block = "By: /s/ J. D. R. Its: CFO Date: 3/16/20 No. 2197 ATTEST: City Secretary \
             APPROVED AS TO FORM: City Attorney for the City of Sugar Land, Texas";
        let thresholds = OcrQualityThresholds::default();
        let stats = NativeTextStats::compute(signature_block, &thresholds);

        assert!(
            stats.word_count >= thresholds.min_words_for_ocr_output_check,
            "fixture must clear the word-count guard so the ratio is what is tested"
        );
        assert!(!is_ocr_recognition_noise(signature_block, &thresholds));
    }

    #[test]
    fn should_keep_a_page_of_tabular_ocr() {
        // A Markdown table's delimiter row is entirely one-character tokens. Scoring the raw
        // Markdown would make good tabular OCR indistinguishable from line-art noise, so this
        // is the veto's most likely false positive and the reason it scores normalized prose.
        let table_page = "\
Annual Report Summary of Operating Results by Region and Quarter

| Region | Quarter | Revenue | Growth |
| --- | --- | --- | --- |
| North | Q1 | 1,240 | 4.2 |
| North | Q2 | 1,310 | 5.6 |
| South | Q1 | 980 | 2.1 |
| South | Q2 | 1,045 | 6.6 |

Revenue is reported in thousands of dollars and growth is year over year.";
        let thresholds = OcrQualityThresholds::default();

        assert!(
            !is_ocr_recognition_noise(table_page, &thresholds),
            "tabular OCR must survive; raw short-word ratio is {:.3}",
            NativeTextStats::compute(table_page, &thresholds).fragmented_word_ratio
        );
    }

    #[test]
    fn should_score_prose_not_markdown_scaffolding() {
        // Pin the mechanism, not just the outcome: the raw text must look worse than the
        // normalized text, or the normalization above is not doing anything.
        let table_page = "| a | b | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n\n\
Each row of the preceding table records one measurement taken during the survey period.";
        let thresholds = OcrQualityThresholds::default();

        let raw = NativeTextStats::compute(table_page, &thresholds).fragmented_word_ratio;
        let normalized = ocr_output_stats(table_page, &thresholds).fragmented_word_ratio;

        assert!(
            normalized < raw,
            "normalization changed nothing: raw {raw:.3} vs normalized {normalized:.3}"
        );
    }

    #[test]
    fn should_read_mean_confidence_from_backend_metadata() {
        use super::mean_text_conf_of;
        let mut m: ahash::AHashMap<std::borrow::Cow<'_, str>, serde_json::Value> = Default::default();

        assert_eq!(
            mean_text_conf_of(&m),
            None,
            "absent key means the backend reported none"
        );

        m.insert("mean_text_conf".into(), serde_json::json!(93));
        assert_eq!(mean_text_conf_of(&m), Some(93.0), "integers must parse");

        m.insert("mean_text_conf".into(), serde_json::json!(57.5));
        assert_eq!(mean_text_conf_of(&m), Some(57.5), "floats must parse");

        // Tesseract returns -1 when it has no confidence to report. Treating that as a
        // score would reject every such page, so it must read as "unavailable" instead.
        m.insert("mean_text_conf".into(), serde_json::json!(-1));
        assert_eq!(mean_text_conf_of(&m), None);
    }

    #[test]
    fn confidence_default_sits_between_the_measured_populations() {
        // Per-page mean confidence measured by xberg over a recorded ordinance
        // (Tesseract 5.5.3): prose 89-95, scanned drawings 36-62. The default must
        // separate them. If a future change moves the default outside that band, this
        // fails before any document is silently re-graded.
        let thresholds = OcrQualityThresholds::default();
        let prose = [95.0, 89.0, 95.0, 95.0, 93.0, 93.0, 94.0, 94.0, 95.0, 92.0];
        let drawings = [36.0, 58.0, 62.0, 58.0, 57.0];

        let worst_prose = prose.iter().cloned().fold(f64::INFINITY, f64::min);
        let best_drawing = drawings.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        assert!(
            best_drawing < thresholds.min_ocr_mean_confidence,
            "a drawing at {best_drawing} would survive the {} floor",
            thresholds.min_ocr_mean_confidence
        );
        assert!(
            thresholds.min_ocr_mean_confidence < worst_prose,
            "prose at {worst_prose} would be rejected by the {} floor",
            thresholds.min_ocr_mean_confidence
        );
    }

    #[test]
    fn should_repair_a_comma_read_for_a_list_period() {
        // Verbatim from the ordinance: `3.` and `4.` came back as `3,` and `4,`, so neither
        // line was a list item any more.
        let text = "3, Maximum number of lots: 9\n4, Minimum lot area: 2,842 sf";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "3. Maximum number of lots: 9\n4. Minimum lot area: 2,842 sf"
        );
    }

    #[test]
    fn should_repair_a_lowercase_l_read_for_a_digit_one() {
        let text = "l. A six-foot (6') pedestrian sidewalk towards Creek Bend Drive";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "1. A six-foot (6') pedestrian sidewalk towards Creek Bend Drive"
        );
    }

    #[test]
    fn should_not_touch_prose_that_merely_starts_with_a_number() {
        // The uppercase requirement is what separates these from list items. Rewriting them
        // would corrupt text the engine read correctly.
        for line in [
            "3, and the remainder of the tract is unrestricted",
            "2024, the year the plat was recorded",
            "1985, when the ordinance was adopted",
        ] {
            assert_eq!(repair_ocr_list_markers(line).as_ref(), line, "rewrote prose: {line}");
        }
    }

    #[test]
    fn should_not_touch_a_year_or_long_number_before_a_capital() {
        // Four digits is not a list marker; the 2-digit cap is what stops this.
        let line = "2019, November of that year saw the plat recorded";
        assert_eq!(repair_ocr_list_markers(line).as_ref(), line);
    }

    #[test]
    fn should_leave_correct_markers_alone_and_avoid_allocating() {
        let text = "1. Land Use: Live/Work Townhomes\n2. Building finishes: Siding";
        let out = repair_ocr_list_markers(text);
        assert!(
            matches!(out, std::borrow::Cow::Borrowed(_)),
            "must not allocate when nothing is broken"
        );
        assert_eq!(out.as_ref(), text);
    }

    #[test]
    fn should_preserve_surrounding_lines_and_trailing_newline() {
        let text = "Section 3. Regulations\n\n3, Maximum number of lots: 9\nunchanged tail\n";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "Section 3. Regulations\n\n3. Maximum number of lots: 9\nunchanged tail\n"
        );
    }

    #[test]
    fn should_map_confusable_letters_to_their_intended_digit() {
        use super::confusable_digit_for_letter;
        assert_eq!(confusable_digit_for_letter('L'), Some('1'));
        assert_eq!(confusable_digit_for_letter('G'), Some('6'));
        assert_eq!(confusable_digit_for_letter('b'), Some('6'));
        assert_eq!(confusable_digit_for_letter('S'), Some('5'));
        assert_eq!(confusable_digit_for_letter('O'), Some('0'));
        assert_eq!(confusable_digit_for_letter('D'), Some('0'));
        assert_eq!(confusable_digit_for_letter('I'), Some('1'));
        assert_eq!(
            confusable_digit_for_letter('A'),
            None,
            "A is a legitimate lettered marker"
        );
    }

    #[test]
    fn should_repair_a_doubled_misread_of_a_digit_one() {
        // `lL.` is the digit `1` split into two mis-read characters; never a valid marker.
        let text = "lL. A six-foot (6') pedestrian sidewalk towards Creek Bend Drive";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "1. A six-foot (6') pedestrian sidewalk towards Creek Bend Drive"
        );
    }

    #[test]
    fn should_repair_a_letter_misread_of_a_digit_inside_a_numeric_run() {
        // Verbatim shape from the ordinance: `G.` between two numeric markers is a mis-read
        // `6.`, not a lettered marker.
        let text = "5. Front setback: 20 feet\nG. Side setback: 10 feet\n7. Rear setback: 15 feet";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "5. Front setback: 20 feet\n6. Side setback: 10 feet\n7. Rear setback: 15 feet"
        );
    }

    #[test]
    fn should_repair_a_letter_misread_with_context_on_only_one_side() {
        // No marker precedes it on the page, but the following marker is numeric.
        let text = "G. Side setback: 10 feet\n7. Rear setback: 15 feet";
        assert_eq!(
            repair_ocr_list_markers(text).as_ref(),
            "6. Side setback: 10 feet\n7. Rear setback: 15 feet"
        );
    }

    #[test]
    fn should_not_touch_a_genuine_lettered_marker_between_lettered_neighbors() {
        // This is the corruption this discriminator exists to prevent: `G.` here is the
        // legitimate 7th item of a lettered list, not a mis-read `6.`.
        let text = "F. Fire lane width: 20 feet\nG. Side setback: 10 feet\nH. Height limit: 35 feet";
        assert_eq!(repair_ocr_list_markers(text).as_ref(), text);
    }

    #[test]
    fn should_not_touch_an_isolated_ambiguous_letter_marker() {
        // No determinable neighbor on either side -- decline to judge rather than guess.
        let text = "G. Side setback: 10 feet";
        assert_eq!(repair_ocr_list_markers(text).as_ref(), text);
    }

    #[test]
    fn legibility_backend_still_rejects_a_page_below_the_floor() {
        // Tesseract's own scale must keep working exactly as before: `Legibility { scale_max:
        // 100.0 }` normalizes to the same fraction as the old unconditional `c < threshold`
        // check did, so this must not regress.
        use super::confidence_gate_rejects;
        let thresholds = OcrQualityThresholds::default();
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 100.0 };

        assert!(
            confidence_gate_rejects(semantics, Some(39.0), thresholds.min_ocr_mean_confidence),
            "a page at confidence 39 (the real ordinance's worst legible page) must still be \
             rejected under Tesseract's own 100-point scale"
        );
        assert!(
            !confidence_gate_rejects(semantics, Some(95.0), thresholds.min_ocr_mean_confidence),
            "clean prose at confidence 95 must not be rejected"
        );
    }

    #[test]
    fn uncalibrated_backend_confidence_never_empties_a_document() {
        // Regression for the sceptre bug: every page of a 16-page recorded ordinance scored
        // between 36 and 74 on sceptre's rescaled `custom_mean` -- entirely below the 75.0
        // default floor, which is tuned for Tesseract's scale -- and applying the floor
        // discarded all 16 pages, emptying the document. An `Uncalibrated` backend's number
        // must never be able to do that: the gate must not apply at all, and a legible page
        // must survive on the text-shape heuristic instead.
        use super::{confidence_gate_applies, confidence_gate_rejects};
        let thresholds = OcrQualityThresholds::default();
        let semantics = crate::plugins::ConfidenceSemantics::Uncalibrated;

        for sceptre_like_confidence in [36.0, 39.0, 57.0, 62.0, 74.0] {
            let confidence = Some(sceptre_like_confidence);
            let rejected_by_confidence =
                confidence_gate_rejects(semantics, confidence, thresholds.min_ocr_mean_confidence);
            let judged_by_confidence = confidence_gate_applies(semantics, confidence);
            assert!(
                !rejected_by_confidence,
                "confidence {sceptre_like_confidence} must never gate an Uncalibrated backend's page"
            );
            let kept = !rejected_by_confidence
                && (judged_by_confidence || !is_ocr_recognition_noise(ORDINANCE_PROSE, &thresholds));
            assert!(
                kept,
                "a legible page (confidence {sceptre_like_confidence}) must survive when the \
                 reporting backend is Uncalibrated"
            );
        }
    }

    #[test]
    fn confidence_gate_respects_scale_max_not_a_hardcoded_100() {
        // scale_max = 10, confidence = 8 -> 80% of scale, above a 75%-of-100 threshold, so
        // this must NOT be rejected. Comparing the raw value 8 directly against the
        // (100-scaled) threshold of 75 -- the old hardcoded-100 assumption -- would wrongly
        // reject it (8 < 75). Only normalizing by the backend's own `scale_max` gets this right.
        use super::confidence_gate_rejects;
        let thresholds = OcrQualityThresholds::default();
        assert_eq!(
            thresholds.min_ocr_mean_confidence, 75.0,
            "test assumes the documented default"
        );
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 10.0 };

        assert!(
            !confidence_gate_rejects(semantics, Some(8.0), thresholds.min_ocr_mean_confidence),
            "8 of a 10-point scale (80%) must clear a 75%-of-scale floor"
        );
        assert!(
            confidence_gate_rejects(semantics, Some(7.0), thresholds.min_ocr_mean_confidence),
            "7 of a 10-point scale (70%) must not clear a 75%-of-scale floor"
        );
    }

    #[test]
    fn pipeline_blend_drops_mean_conf_term_for_non_legibility_stage() {
        // `extract_with_ocr` only ever reports `Some(mean_conf)` when its backend is
        // `Legibility`; for anything else it reports `None`. The blend must then fall back
        // to the text-shape score alone rather than averaging in an incomparable number.
        use super::pipeline_stage_score;
        let text_score = 0.62;

        assert_eq!(
            pipeline_stage_score(text_score, None),
            text_score,
            "a non-Legibility stage's score must be the text score alone, unblended"
        );
        assert_ne!(
            pipeline_stage_score(text_score, Some(0.1)),
            text_score,
            "a Legibility stage's reported confidence must still influence the score"
        );
    }

    #[test]
    fn resolve_confidence_semantics_reads_the_backend_object_not_its_name() {
        // A backend named to look calibrated but whose `confidence_semantics()` says
        // otherwise: the gate must trust the object, not a name-based guess. If this ever
        // regresses to matching on the name, this backend would be wrongly treated as
        // Legibility and could empty a document exactly like the sceptre bug did.
        use crate::core::config::OcrConfig;
        use crate::plugins::{ConfidenceSemantics, OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        struct DeceptivelyNamedBackend;

        #[async_trait::async_trait]
        impl OcrBackend for DeceptivelyNamedBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument::default())
            }
            fn confidence_semantics(&self) -> ConfidenceSemantics {
                ConfidenceSemantics::Uncalibrated
            }
        }

        impl Plugin for DeceptivelyNamedBackend {
            fn name(&self) -> &str {
                "tesseract-lookalike"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend: Arc<dyn OcrBackend> = Arc::new(DeceptivelyNamedBackend);
        let semantics = super::resolve_confidence_semantics(Some(&backend), "tesseract-lookalike");
        assert_eq!(
            semantics,
            ConfidenceSemantics::Uncalibrated,
            "must read the backend's own confidence_semantics(), not its name"
        );
    }

    #[test]
    fn should_keep_empty_output_out_of_the_veto() {
        // Blank pages are rejected earlier by their own check; the veto must not also claim
        // them, or the warning it emits would be wrong about why the page is empty.
        let thresholds = OcrQualityThresholds::default();
        assert!(!is_ocr_recognition_noise("", &thresholds));
        assert!(!is_ocr_recognition_noise("   \n\n  ", &thresholds));
    }

    #[test]
    fn should_respect_a_configured_threshold() {
        // Raising the bar above the fixture's ratio must keep the page.
        let permissive = OcrQualityThresholds {
            max_ocr_output_fragmented_word_ratio: 0.99,
            ..Default::default()
        };
        assert!(!is_ocr_recognition_noise(PLAT_DRAWING_NOISE, &permissive));

        // Lowering it below prose must reject even prose — proving the knob is live and
        // that the default, not the code path, is what protects real text.
        let strict = OcrQualityThresholds {
            max_ocr_output_fragmented_word_ratio: 0.01,
            ..Default::default()
        };
        assert!(is_ocr_recognition_noise(ORDINANCE_PROSE, &strict));
    }

    #[test]
    fn should_separate_the_two_fixtures_with_margin() {
        // The default sits between them. If a future change narrows this gap, the veto is
        // no longer safe to apply and this test should fail before anything ships.
        let thresholds = OcrQualityThresholds::default();
        let noise = NativeTextStats::compute(PLAT_DRAWING_NOISE, &thresholds).fragmented_word_ratio;
        let prose = NativeTextStats::compute(ORDINANCE_PROSE, &thresholds).fragmented_word_ratio;

        assert!(
            noise - prose > 0.15,
            "separation collapsed: noise {noise:.3} vs prose {prose:.3}"
        );
        assert!(prose < thresholds.max_ocr_output_fragmented_word_ratio);
        assert!(noise >= thresholds.max_ocr_output_fragmented_word_ratio);
    }
}
