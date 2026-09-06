#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use crate::core::config::OcrQualityThresholds;

/// Minimum average non-whitespace characters per page for extracted text to be treated as
/// substantive. At or above this, prose-tuned quality checks (fragmentation, avg word length,
/// consecutive-repeat ratio) are skipped so legitimately non-prose content — numeric tables,
/// formula pages, sparse forms — is not misclassified as needing OCR (issue #1176). Corruption
/// checks (empty, no-alphanumerics, garbage chars, critical fragmentation) still always apply.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const MIN_AVG_NON_WHITESPACE_TO_TRUST: f64 = 150.0;
/// Inclusive start of the Unicode Private Use Area (BMP: U+E000-U+F8FF). Codepoints here
/// have no standard meaning; a font's glyph-index-to-character mapping that resolves into
/// this range signals an undecodable text layer rather than real text (issue #1254).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const PUA_RANGE_START: u32 = 0xE000;
/// Inclusive end of the Unicode Private Use Area (BMP).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const PUA_RANGE_END: u32 = 0xF8FF;
/// OCR may legitimately clean a native text layer, but recovering less than half of an
/// independently healthy page is evidence of destructive recognition rather than cleanup. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) const MIN_OCR_NATIVE_ALNUM_RETENTION_RATIO: f64 = 0.5;
/// Returns `true` for characters that indicate a broken glyph-to-Unicode mapping rather
/// than legible text: Unicode Private Use Area codepoints (a common fallback target for
/// undecodable CID/glyph indices), the replacement character (U+FFFD), and non-whitespace
/// control characters. Ordinary symbols, punctuation, and emoji are unaffected.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn is_undecodable_char(ch: char) -> bool {
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
        && !decision.fallback
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
        // A token with no alphabetic content cannot be a fragmented *word* — it is
        // structurally-expected punctuation (a `- - - -` section divider, a `:` speaker-turn
        // marker) or a left-margin line number. Counting those as "short words" is exactly
        // what makes a court-transcript page indistinguishable from line-art noise: both are
        // dense with 1-2 character tokens, but the transcript's are dividers and digits, not
        // truncated recognition. Excluded tokens leave the denominator too, mirroring how a
        // Markdown table's delimiter row is dropped entirely from the scoring input rather
        // than kept and merely un-counted as "short" (see `normalize_markdown_for_scoring`).
        // Keeping them in the denominator while excluding them from the numerator would just
        // move the false rejection to a different shape of transcript-heavy page. ~keep
        //
        // The `>= 10` guard deliberately stays on the TOTAL token count, not the scorable
        // one: it asks "is there enough on this page to judge at all", which is a question
        // about the page, not about how many tokens survive the filter. Moving it to the
        // scorable count would silently abstain on dense line-art whose fragments happen to
        // be mostly punctuation -- the exact pages the veto exists for. ~keep
        let scorable_words: Vec<&&str> = words.iter().filter(|w| w.chars().any(char::is_alphabetic)).collect();
        let fragmented_word_ratio = if words.len() >= 10 && !scorable_words.is_empty() {
            let short_count = scorable_words.iter().filter(|w| w.len() <= 2).count();
            short_count as f64 / scorable_words.len() as f64
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
    evaluate_native_text_for_ocr_with_garbage_threshold(native_text, page_count, thresholds, true)
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn evaluate_native_text_for_ocr_with_garbage_threshold(
    native_text: &str,
    page_count: Option<u32>,
    thresholds: &OcrQualityThresholds,
    apply_absolute_garbage_threshold: bool,
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
    let pages = page_count.unwrap_or(1).max(1);
    let avg_non_whitespace = stats.non_whitespace as f64 / f64::from(pages);
    let avg_alnum = stats.alnum as f64 / f64::from(pages);

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

    let has_excessive_garbage =
        apply_absolute_garbage_threshold && stats.garbage_char_count >= thresholds.min_garbage_chars;

    let definitive_failure = stats.non_whitespace == 0
        || stats.alnum == 0
        || has_excessive_garbage
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
pub(super) fn normalize_markdown_for_scoring(text: &str) -> String {
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
pub(super) const ENABLE_WIDENED_OCR_LIST_MARKER_REPAIR: bool = true;
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
pub(super) enum LineMarkerKind {
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
pub(super) fn classify_marker_line(line: &str) -> LineMarkerKind {
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
pub(super) fn confusable_digit_for_letter(ch: char) -> Option<char> {
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
pub(super) fn nearest_marker_is_digit(kind: &LineMarkerKind) -> Option<bool> {
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
pub(super) fn ambiguous_marker_is_numeric_context(kinds: &[LineMarkerKind], index: usize) -> bool {
    let before = kinds[..index].iter().rev().find_map(nearest_marker_is_digit);
    let after = kinds[index + 1..].iter().find_map(nearest_marker_is_digit);
    matches!(
        (before, after),
        (Some(true), Some(true)) | (Some(true), None) | (None, Some(true))
    )
}
/// Rewrite a line already decided to be repaired, per its classified kind.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn repaired_marker_line(line: &str, kind: &LineMarkerKind) -> String {
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
/// The backend-native-scale confidence floor a page's confidence must clear, or `false` if
/// this backend's confidence cannot be used as a calibrated diagnostic at all.
///
/// Only [`ConfidenceSemantics::Legibility`] is normalized (by its `scale_max`) and compared
/// against the ABSOLUTE 0-100 `min_ocr_mean_confidence` threshold. `Uncalibrated` and `None`
/// never trigger here because their number, if any, does not mean legibility.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn confidence_gate_rejects(
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
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Debug, Clone, Copy)]
pub(super) struct OcrRecognitionNoiseDecision {
    pub(super) low_confidence: bool,
    pub(super) fragmented_noise: bool,
}
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
impl OcrRecognitionNoiseDecision {
    fn suspected(self) -> bool {
        self.low_confidence || self.fragmented_noise
    }
}
/// Apply one recognition-noise policy to every PDF OCR route.
///
/// Confidence is consulted only when the producing backend declares a calibrated legibility
/// scale. Fragmentation remains an independent signal so warnings report every reason that fired. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn ocr_recognition_noise_decision(
    content: &str,
    thresholds: &OcrQualityThresholds,
    semantics: crate::plugins::ConfidenceSemantics,
    confidence: Option<f64>,
) -> OcrRecognitionNoiseDecision {
    let low_confidence = confidence_gate_rejects(semantics, confidence, thresholds.min_ocr_mean_confidence);
    let fragmented_noise = is_ocr_recognition_noise(content, thresholds);
    OcrRecognitionNoiseDecision {
        low_confidence,
        fragmented_noise,
    }
}
/// Tesseract's mean confidence for a page, 0-100, if the backend reported one.
///
/// Written by `perform_ocr` from `api.mean_text_conf()`. Backends that do not report it
/// (and Tesseract itself, when it read nothing) simply yield `None`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn mean_text_conf_of(
    metadata: &ahash::AHashMap<std::borrow::Cow<'_, str>, serde_json::Value>,
) -> Option<f64> {
    let value = metadata.get("mean_text_conf")?;
    let conf = value.as_f64().or_else(|| value.as_i64().map(|v| v as f64))?;
    // -1 is Tesseract's "no confidence available" sentinel.
    (conf >= 0.0).then_some(conf)
}
/// The number of OCR'd words a backend retained on a page, if it reported one.
///
/// Written into the same metadata map as `mean_text_conf` by
/// `insert_retained_word_confidence_metadata` (`ocr::processor::execution`). Backends that
/// never report it simply yield `None`, which callers read as "unknown", not as zero words.
/// The `u64` is narrowed by saturation rather than cast, so an implausibly large count
/// cannot silently wrap into a small one. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn word_count_of(metadata: &ahash::AHashMap<std::borrow::Cow<'_, str>, serde_json::Value>) -> Option<u32> {
    let count = metadata.get("word_count")?.as_u64()?;
    Some(u32::try_from(count).unwrap_or(u32::MAX))
}
/// Build the page-level OCR confidence summary attached to [`crate::types::page::PageContent`].
///
/// Only [`crate::plugins::ConfidenceSemantics::Legibility`] yields a `score`: its raw value is
/// normalized by `scale_max` into `0.0..=1.0` and clamped, mirroring [`confidence_gate_rejects`]'s
/// normalization. A non-positive `scale_max` would divide into NaN/infinity, so it is guarded
/// and treated the same as "no calibrated scale" rather than propagating a broken number.
///
/// `Uncalibrated` and `None` semantics, and a missing `raw_confidence` (no words were scored),
/// all yield `score: None` -- but always with the real `word_count` and `backend` populated, so
/// a caller can still see that the page WAS OCR'd and by whom even without a legibility number.
/// `Some(0.0)` is never used as a substitute for "no data": zero confidence and no data are
/// different facts, and collapsing them would make a genuinely illegible page indistinguishable
/// from one nobody scored.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn page_ocr_confidence(
    semantics: crate::plugins::ConfidenceSemantics,
    raw_confidence: Option<f64>,
    word_count: u32,
    backend: &str,
) -> Option<crate::types::page::PageOcrConfidence> {
    let score = match semantics {
        crate::plugins::ConfidenceSemantics::Legibility { scale_max } if scale_max > 0.0 => {
            raw_confidence.map(|raw| (raw / scale_max).clamp(0.0, 1.0))
        }
        _ => None,
    };
    Some(crate::types::page::PageOcrConfidence {
        score,
        word_count,
        backend: backend.to_string(),
    })
}
/// Statistics for judging an OCR result, scored over prose rather than Markdown scaffolding.
///
/// A table's delimiter rows (`| --- | --- |`) are entirely one-character tokens, so scoring the
/// raw Markdown makes a page of perfectly good tabular OCR look exactly like line-art noise.
/// [`compute_quality_score`] already normalizes for this (#1341); the veto has to as well, or a
/// scanned table becomes its most likely false positive.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn ocr_output_stats(text: &str, thresholds: &OcrQualityThresholds) -> NativeTextStats {
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
/// A 0.09 separation is not something to threshold on. A diagnostic needs the signal that
/// actually discriminates, not an average.
///
/// That signal is the short-word ratio, which [`NativeTextStats`] already computes. On the
/// same document prose ran 0.04-0.28 and the drawings 0.42-0.47.
///
/// This is deliberately conservative: it declines to judge pages with too few words to make
/// the ratio meaningful, and its threshold sits in the middle of the measured gap rather than
/// at either edge.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn is_ocr_recognition_noise(text: &str, thresholds: &OcrQualityThresholds) -> bool {
    let stats = ocr_output_stats(text, thresholds);
    if stats.word_count < thresholds.min_words_for_ocr_output_check {
        return false;
    }
    stats.fragmented_word_ratio >= thresholds.max_ocr_output_fragmented_word_ratio
}
/// Whether a page's Tesseract dictionary-invalid-word ratio crosses the configured
/// threshold, supplementing [`is_ocr_recognition_noise`] rather than replacing it.
///
/// `dict_invalid_word_ratio` is `None` for every non-Tesseract backend (they never report
/// this ratio at all) and for Tesseract pages too short to make it meaningful (see
/// `dictionary_invalid_word_ratio` in `ocr::processor::execution`). Absence must never be
/// read as `0.0`: this function only rejects on a *reported* ratio, so a backend or page
/// that cannot compute the signal simply never trips this particular check.
///
/// The default threshold (see `OcrQualityThresholds::max_ocr_output_dict_invalid_word_ratio`)
/// disables this check until it is calibrated.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn is_dictionary_invalid_noise(
    dict_invalid_word_ratio: Option<f64>,
    thresholds: &OcrQualityThresholds,
) -> bool {
    dict_invalid_word_ratio.is_some_and(|ratio| ratio > thresholds.max_ocr_output_dict_invalid_word_ratio)
}
/// The numeric evidence behind a page's recognition-noise verdict, carried out of
/// [`accept_or_reject_ocr_page`] alongside its accept/reject outcome.
///
/// The blended `compute_quality_score` cannot discriminate recognition noise (median 0.924,
/// p05 0.807 across OCR-routed pages -- thresholds from 0.50 to 0.75 all escalate the same
/// 1.1% of pages, then cliff to 88% at 0.95). This per-page signal, by contrast, separates a
/// 1,061-page corpus cleanly (median dictionary-valid-word ratio 0.772 for documents where it
/// fires on >=15% of pages, vs. 0.928 for the rest) but was previously discarded one frame
/// before the accept decision that needs it. Plumbed through, not yet gated on. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct OcrPageNoiseVerdict {
    pub(crate) page_index: usize,
    pub(crate) low_confidence: bool,
    pub(crate) fragmented_noise: bool,
    pub(crate) dictionary_noise: bool,
    pub(crate) fragmented_word_ratio: f64,
    pub(crate) word_count: usize,
    /// The raw, backend-native confidence actually compared by the confidence gate --
    /// not document-level and not scale-normalized.
    pub(crate) mean_confidence: Option<f64>,
    pub(crate) dict_invalid_word_ratio: Option<f64>,
    pub(crate) discarded: bool,
}
/// Outcome of [`accept_or_reject_ocr_page`]: the (possibly discarded) page text, whether it
/// was discarded, and -- when recognition noise was suspected -- the numeric verdict behind
/// that call, so a caller can observe the signal without recomputing stats.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) struct OcrPageAcceptance {
    pub(super) content: String,
    pub(super) discarded: bool,
    pub(super) verdict: Option<OcrPageNoiseVerdict>,
}
/// Assess a page's OCR text, report suspected recognition noise, and optionally discard it.
///
/// The boolean verdict is destructive only when `discard_suspected_ocr_noise` is enabled.
/// Blank pages carry neither a warning nor a destructive verdict, and `verdict` is `None`
/// whenever no warning fires -- mirroring the `warnings` accumulator this function already
/// writes into.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn accept_or_reject_ocr_page(
    page_index: usize,
    content: String,
    thresholds: &OcrQualityThresholds,
    warnings: &mut Vec<crate::types::ProcessingWarning>,
    dict_invalid_word_ratio: Option<f64>,
    confidence_semantics: crate::plugins::ConfidenceSemantics,
    confidence: Option<f64>,
) -> OcrPageAcceptance {
    if content.trim().is_empty() {
        return OcrPageAcceptance {
            content,
            discarded: false,
            verdict: None,
        };
    }
    let recognition_noise = ocr_recognition_noise_decision(&content, thresholds, confidence_semantics, confidence);
    let dictionary_noise = is_dictionary_invalid_noise(dict_invalid_word_ratio, thresholds);
    if !recognition_noise.suspected() && !dictionary_noise {
        return OcrPageAcceptance {
            content,
            discarded: false,
            verdict: None,
        };
    }
    let discarded = thresholds.discard_suspected_ocr_noise;
    let stats = ocr_output_stats(&content, thresholds);
    tracing::warn!(
        page = page_index + 1,
        words = stats.word_count,
        fragmented_word_ratio = stats.fragmented_word_ratio,
        threshold = thresholds.max_ocr_output_fragmented_word_ratio,
        dict_invalid_word_ratio = dict_invalid_word_ratio,
        dict_invalid_word_ratio_threshold = thresholds.max_ocr_output_dict_invalid_word_ratio,
        mean_text_confidence = confidence,
        low_confidence = recognition_noise.low_confidence,
        rejected_by_dictionary_signal = dictionary_noise,
        discarded,
        "OCR output triggered recognition-noise diagnostics"
    );
    // Name only the signals that actually fired. Leading with the fragmentation numbers
    // unconditionally would report a ratio *below* its own threshold as the reason whenever
    // the dictionary signal is what rejected the page.
    let mut reasons: Vec<String> = Vec::new();
    if recognition_noise.low_confidence {
        let scale_max = match confidence_semantics {
            crate::plugins::ConfidenceSemantics::Legibility { scale_max } => scale_max,
            _ => 100.0,
        };
        reasons.push(format!(
            "mean confidence {:.0}% of scale is below threshold {:.0}%",
            (confidence.unwrap_or_default() / scale_max) * 100.0,
            thresholds.min_ocr_mean_confidence
        ));
    }
    if recognition_noise.fragmented_noise {
        reasons.push(format!(
            "{:.0}% of {} words are 1-2 characters, threshold {:.0}%",
            stats.fragmented_word_ratio * 100.0,
            stats.word_count,
            thresholds.max_ocr_output_fragmented_word_ratio * 100.0
        ));
    }
    if let Some(ratio) = dict_invalid_word_ratio
        && dictionary_noise
    {
        reasons.push(format!(
            "{:.0}% of dictionary-checkable words are dictionary-invalid, threshold {:.0}%",
            ratio * 100.0,
            thresholds.max_ocr_output_dict_invalid_word_ratio * 100.0
        ));
    }
    warnings.push(crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("ocr"),
        message: std::borrow::Cow::Owned(if discarded {
            format!(
                "Page {} produced suspected OCR recognition noise ({}); its text was discarded \
                 because discard_suspected_ocr_noise is enabled.",
                page_index + 1,
                reasons.join("; ")
            )
        } else {
            format!(
                "Page {} produced suspected OCR recognition noise ({}); its text was retained. \
                 Set discard_suspected_ocr_noise to true to discard suspected noise.",
                page_index + 1,
                reasons.join("; ")
            )
        }),
    });
    let verdict = Some(OcrPageNoiseVerdict {
        page_index,
        low_confidence: recognition_noise.low_confidence,
        fragmented_noise: recognition_noise.fragmented_noise,
        dictionary_noise,
        fragmented_word_ratio: stats.fragmented_word_ratio,
        word_count: stats.word_count,
        mean_confidence: confidence,
        dict_invalid_word_ratio,
        discarded,
    });
    if discarded {
        OcrPageAcceptance {
            content: String::new(),
            discarded: true,
            verdict,
        }
    } else {
        OcrPageAcceptance {
            content,
            discarded: false,
            verdict,
        }
    }
}
/// The text that quality scoring actually judges: the prose content with Markdown scaffolding
/// stripped (#1341), falling back to the raw trimmed text when normalization leaves nothing
/// (e.g. a table-only fragment).
///
/// Shared so [`compute_quality_score`] and the F46 density guard cannot disagree about what
/// counts as content. Judging normalized text in one and raw text in the other let a runaway
/// table-separator row inflate the density denominator while the meaningful-word count stayed
/// flat, vetoing a good VLM candidate -- #1341's Markdown-penalizes-VLM bias reintroduced
/// through an un-normalized side door. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(super) fn scoring_input(trimmed: &str) -> std::borrow::Cow<'_, str> {
    let normalized = normalize_markdown_for_scoring(trimmed);
    if normalized.trim().is_empty() {
        std::borrow::Cow::Borrowed(trimmed)
    } else {
        std::borrow::Cow::Owned(normalized)
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn compute_quality_score(text: &str, thresholds: &OcrQualityThresholds) -> f64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }

    let input = scoring_input(trimmed);
    let stats = NativeTextStats::compute(&input, thresholds);

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
pub(super) fn pipeline_stage_score(text_score: f64, mean_conf: Option<f64>) -> f64 {
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

    let boundary_count_matches_pages = page_count.is_none_or(|count| count as usize == boundaries.len());
    let all_boundaries_are_valid = boundaries.iter().all(|boundary| {
        boundary.byte_start <= boundary.byte_end
            && native_text.is_char_boundary(boundary.byte_start)
            && native_text.is_char_boundary(boundary.byte_end)
    });
    let boundaries_are_ordered = boundaries.windows(2).all(|pair| pair[0].byte_end <= pair[1].byte_start);
    let page_numbers_are_complete = boundaries.iter().enumerate().all(|(index, boundary)| {
        usize::try_from(boundary.page_number).is_ok_and(|page_number| page_number == index + 1)
    });
    let all_garbage_is_covered = all_boundaries_are_valid
        && boundaries_are_ordered
        && boundaries
            .iter()
            .map(|boundary| {
                native_text[boundary.byte_start..boundary.byte_end]
                    .chars()
                    .filter(|character| *character == '\u{FFFD}')
                    .count()
            })
            .sum::<usize>()
            == native_text.chars().filter(|character| *character == '\u{FFFD}').count();
    let can_defer_absolute_garbage_threshold = boundaries.len() > 1
        && boundary_count_matches_pages
        && all_boundaries_are_valid
        && boundaries_are_ordered
        && page_numbers_are_complete
        && all_garbage_is_covered;

    // Defer the aggregate absolute count only when every page can be evaluated; otherwise
    // the document-level check is the only way to preserve the configured fallback threshold. ~keep
    let mut document_decision = evaluate_native_text_for_ocr_with_garbage_threshold(
        native_text,
        page_count,
        thresholds,
        !can_defer_absolute_garbage_threshold,
    );

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
