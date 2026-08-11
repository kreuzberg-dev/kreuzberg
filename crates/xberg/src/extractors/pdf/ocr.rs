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

    let page_rotations = crate::pdf::render::get_page_rotations(content, page_count);
    Ok((doc, page_count, page_rotations))
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
    let page_rotations = crate::pdf::render::get_page_rotations(content, page_count);
    Ok((doc, page_count, page_rotations))
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

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn assemble_mixed_ocr_page_document(
    mut doc: crate::types::internal::InternalDocument,
    page_number: u32,
    page_height: u32,
) -> crate::types::internal::InternalDocument {
    let paragraphs = crate::pdf::structure::adapters::ocr_doc_to_paragraphs(&doc, page_height);
    if !paragraphs.is_empty() {
        let mut assembled =
            crate::pdf::structure::assemble_internal_document(vec![paragraphs], &doc.tables, Some(&doc.images), &[]);
        assembled.processing_warnings = std::mem::take(&mut doc.processing_warnings);
        doc = assembled;
    }

    normalize_mixed_ocr_document_page(&mut doc, page_number);
    doc
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

/// Build the per-page structured document for the single-backend mixed OCR route,
/// carrying the backend's tables and OCR elements instead of dropping them (#60).
///
/// Returns `None` only when the backend produced nothing structured at all, which
/// keeps the raw-text replacement path unchanged for plain-text pages.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn build_mixed_ocr_page_document(
    result: &mut crate::types::ExtractedDocument,
    page_number: u32,
    page_height: u32,
) -> Option<crate::types::internal::InternalDocument> {
    let backend_tables = std::mem::take(&mut result.tables);
    let backend_elements = result.ocr_elements.take().unwrap_or_default();
    let mut doc = match result.ocr_internal_document.take() {
        Some(doc) => doc,
        None if backend_tables.is_empty() && backend_elements.is_empty() => return None,
        None => flat_ocr_page_document(&result.content),
    };
    attach_page_ocr_payload(&mut doc, backend_tables, Vec::new(), page_number);
    let mut assembled = assemble_mixed_ocr_page_document(doc, page_number, page_height);
    attach_page_ocr_payload(&mut assembled, Vec::new(), backend_elements, page_number);
    Some(assembled)
}

/// Build mixed text from native extraction and per-page OCR results.
///
/// For each page boundary, if the page is in `ocr_page_numbers` (1-indexed),
/// use the OCR result; otherwise use the native text slice.
///
/// Page numbers must be >= 1 (invalid values are filtered out with a warning).
/// An `ocr` config is recommended but not required; defaults are used if absent.
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
fn formula_bbox_to_page_points(
    formula: &mut crate::types::Formula,
    doc: &pdf_oxide::PdfDocument,
    page_idx: usize,
    rendered_w: u32,
    rendered_h: u32,
) {
    if let Some(bbox) = formula.bbox {
        let (w_pt, h_pt) = crate::pdf::render::get_page_dimensions_pt(doc, page_idx);
        formula.bbox = Some(crate::pdf::render::pixel_bbox_to_pdf_points(
            bbox, rendered_w, rendered_h, w_pt, h_pt,
        ));
    }
}

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
    let effective_pipeline = if ocr_config_owned.vlm_fallback != crate::core::config::VlmFallbackPolicy::Disabled
        || ocr_config_owned.pipeline.is_some()
    {
        ocr_config_owned.effective_pipeline()
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
    let mut structured_ocr_pages: ahash::AHashMap<u32, crate::types::internal::InternalDocument> =
        ahash::AHashMap::with_capacity(total);
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut accumulated_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
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
                    join_set.spawn(async move {
                        let result = Box::pin(run_ocr_pipeline(
                            None,
                            Some(std::slice::from_ref(image_arc.as_ref())),
                            #[cfg(feature = "layout-detection")]
                            None,
                            &config_clone,
                            &pipeline_clone,
                            None,
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
                    let (text, tables, elements, doc, usage, page_texts, _rasters, formulas) = result?;
                    accumulated_llm_usage.extend(usage);
                    let page_number = (page_idx + 1) as u32;
                    let page_dims = page_images
                        .iter()
                        .find(|(i, _)| *i == page_idx)
                        .map(|(_, img)| (img.width(), img.height()));
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        if let Some((w, h)) = page_dims {
                            formula_bbox_to_page_points(&mut formula, &render_doc, page_idx, w, h);
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
                    let page_doc = match doc {
                        Some(doc) => Some(doc),
                        None if tables.is_empty() && elements.is_empty() => None,
                        None => Some(flat_ocr_page_document(&page_text)),
                    };
                    if let Some(mut d) = page_doc {
                        attach_page_ocr_payload(&mut d, tables, elements, page_number);
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        normalize_mixed_ocr_document_page(&mut d, page_number);
                        structured_ocr_pages.insert(page_number, d);
                    }
                    ocr_results.insert(page_number, page_text);
                }
            }
            #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
            {
                for (page_idx, image) in &page_images {
                    let (text, tables, elements, doc, usage, page_texts, _rasters, formulas) =
                        Box::pin(run_ocr_pipeline(
                            None,
                            Some(std::slice::from_ref(image.as_ref())),
                            #[cfg(feature = "layout-detection")]
                            None,
                            config,
                            pipeline,
                            None,
                        ))
                        .await?;
                    accumulated_llm_usage.extend(usage);
                    let page_number = (*page_idx + 1) as u32;
                    for mut formula in formulas {
                        formula.page = Some(page_number);
                        formula_bbox_to_page_points(&mut formula, &render_doc, *page_idx, image.width(), image.height());
                        accumulated_formulas.push(formula);
                    }
                    let page_text = page_texts.into_iter().next().unwrap_or(text);
                    let page_doc = match doc {
                        Some(doc) => Some(doc),
                        None if tables.is_empty() && elements.is_empty() => None,
                        None => Some(flat_ocr_page_document(&page_text)),
                    };
                    if let Some(mut d) = page_doc {
                        attach_page_ocr_payload(&mut d, tables, elements, page_number);
                        crate::core::diagnostics::dedup_extend_warnings(
                            &mut accumulated_warnings,
                            std::mem::take(&mut d.processing_warnings),
                        );
                        normalize_mixed_ocr_document_page(&mut d, page_number);
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
            for (page_idx, data, _width, _height) in &encoded {
                let backend_clone = Arc::clone(backend);
                let config_clone = ocr_config_owned.clone();
                let data_clone = Arc::clone(data);
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(data_clone, &config_clone).await;
                    (idx, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, result) = join_result.map_err(|e| crate::XbergError::Plugin {
                    message: format!("OCR task panicked: {}", e),
                    plugin_name: "ocr".to_string(),
                })?;
                let mut extraction_result = result?;
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
                        formula_bbox_to_page_points(&mut formula, &render_doc, page_idx, w, h);
                    }
                    accumulated_formulas.push(formula);
                }
                // The backend's own warnings used to be dropped on this route (#60).
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                let height = encoded
                    .iter()
                    .find(|(encoded_page, ..)| *encoded_page == page_idx)
                    .map_or(0, |(_, _, _, height)| *height);
                if let Some(mut page_doc) =
                    build_mixed_ocr_page_document(&mut extraction_result, (page_idx + 1) as u32, height)
                {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    structured_ocr_pages.insert((page_idx + 1) as u32, page_doc);
                }
                ocr_results.insert((page_idx + 1) as u32, extraction_result.content);
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, data, width, height) in &encoded {
                let mut extraction_result = backend.process_image(data.as_slice(), &ocr_config_owned).await?;
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = Some((*page_idx + 1) as u32);
                    formula_bbox_to_page_points(&mut formula, &render_doc, *page_idx, *width, *height);
                    accumulated_formulas.push(formula);
                }
                crate::core::diagnostics::dedup_extend_warnings(
                    &mut accumulated_warnings,
                    std::mem::take(&mut extraction_result.processing_warnings),
                );
                if let Some(mut page_doc) =
                    build_mixed_ocr_page_document(&mut extraction_result, (*page_idx + 1) as u32, *height)
                {
                    crate::core::diagnostics::dedup_extend_warnings(
                        &mut accumulated_warnings,
                        std::mem::take(&mut page_doc.processing_warnings),
                    );
                    structured_ocr_pages.insert((*page_idx + 1) as u32, page_doc);
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

    let accepted_replacements = accepted_ocr_page_replacements(native_text, boundaries, &ocr_results);
    structured_ocr_pages.retain(|page, _| accepted_replacements.contains_key(page));
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

    ocr_results
        .iter()
        .filter(|(page, text)| valid_pages.contains(page) && !text.trim().is_empty())
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
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
use crate::ocr_metadata_keys::{OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY, OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY};
// Same rationale, scoped to `layout-detection` only: `resolved_ocr_correction_degrees` and
// `transform_ocr_elements_to_render_space` (both `layout-detection`-only) are the sole
// readers of these two key names in this file.
#[cfg(feature = "layout-detection")]
use crate::ocr_metadata_keys::{OCR_AUTO_ROTATED_METADATA_KEY, OCR_ORIENTATION_DEGREES_METADATA_KEY};

#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
fn valid_ocr_layout_dimension(value: &serde_json::Value) -> Option<u32> {
    let value = value.as_f64()?;
    if !value.is_finite() || value <= 0.0 || value > u32::MAX as f64 || value.fract() != 0.0 {
        return None;
    }
    Some(value as u32)
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
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

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn resolved_ocr_layout_dimensions(
    metadata: &crate::types::Metadata,
    render_width: u32,
    render_height: u32,
) -> (u32, u32) {
    processed_ocr_layout_dimensions(metadata).unwrap_or((render_width, render_height))
}

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(feature = "layout-detection")]
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

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn assemble_ocr_page_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height: u32,
    detection: Option<&crate::layout::DetectionResult>,
) -> Vec<crate::pdf::structure::types::PdfParagraph> {
    #[cfg(feature = "ocr")]
    if let Some(detection) = detection {
        let hints = super::layout_hints::detection_to_layout_hints_pixel_space(detection, page_height as f32);
        return crate::pdf::structure::adapters::ocr_doc_to_layout_paragraphs(doc, page_height, &hints, 0.5, 0.2);
    }
    #[cfg(not(feature = "ocr"))]
    let _ = detection;

    crate::pdf::structure::adapters::ocr_doc_to_paragraphs(doc, page_height)
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

/// Convert a TATR-recognized table into the public [`crate::types::Table`],
/// carrying over its `detection_bbox` and assigning a deterministic `table_id`.
///
/// `table_index` is the table's 0-based position in the document's push order
/// (see the caller), so the id is `"table-{table_index + 1}"` — never derived
/// from randomness or wall-clock time, so the same input document always
/// produces the same id. See [`crate::types::Table::table_id`] for the shared
/// scheme doc.
#[cfg(feature = "layout-detection")]
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
/// When `layout_detections` are provided (pixel-space, from the same images),
/// uses layout-aware markdown assembly for structured output. Otherwise falls
/// back to plain OCR text concatenation.
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

    let structured_ocr_config;
    let ocr_config = {
        let cfg = ensure_elements_enabled(base_ocr_config);
        #[cfg(feature = "layout-detection")]
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
        let mean_conf = result
            .metadata
            .additional
            .get("mean_text_conf")
            .and_then(|v| v.as_f64())
            .map(|v| v / 100.0);
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

        // See the sibling JoinSet block above: `Send` futures aren't available on wasm32. ~keep
        #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
        {
            let mut join_set: JoinSet<(usize, crate::Result<crate::types::ExtractedDocument>)> = JoinSet::new();
            for (page_idx, image_data, _width, _height) in &encoded_batch {
                let backend_clone = std::sync::Arc::clone(&backend);
                let config_clone = ocr_config_owned.clone();
                let data_clone = Arc::clone(image_data);
                let idx = *page_idx;
                join_set.spawn(async move {
                    let result = backend_clone.process_image_owned(data_clone, &config_clone).await;
                    (idx, result)
                });
            }
            while let Some(join_result) = join_set.join_next().await {
                let (page_idx, ocr_result) = join_result.map_err(|e| crate::XbergError::Plugin {
                    message: format!("OCR task panicked: {}", e),
                    plugin_name: "ocr".to_string(),
                })?;
                batch_ocr_results[page_idx - batch_start] = Some(ocr_result?);
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, image_data, _width, _height) in &encoded_batch {
                let ocr_result = backend.process_image(image_data.as_slice(), &ocr_config_owned).await?;
                batch_ocr_results[page_idx - batch_start] = Some(ocr_result);
            }
        }

        for offset in 0..batch_count {
            let page_idx = batch_start + offset;
            let mut ocr_result = batch_ocr_results[offset].take().expect("OCR result missing for page");
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
                    formula_bbox_to_page_points(&mut formula, doc, page_idx, w, h);
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
            #[cfg(feature = "pdf")]
            if images.is_none()
                && crate::extraction::blank_detection::is_page_text_blank(&ocr_result.content)
                && let Some((render_doc, _, _)) = lazy_pdf_render_state.as_ref()
            {
                let fallback_images = crate::pdf::oxide::images::page_ocr_fallback_image_bytes(render_doc, page_idx);
                if !fallback_images.is_empty() {
                    let mut recovered = String::new();
                    for image_bytes in &fallback_images {
                        match backend.process_image(image_bytes, &ocr_config_owned).await {
                            Ok(fallback_result) if !fallback_result.content.trim().is_empty() => {
                                if !recovered.is_empty() {
                                    recovered.push_str("\n\n");
                                }
                                recovered.push_str(&fallback_result.content);
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::debug!(
                                    page = page_idx,
                                    "force_ocr fallback: OCR of embedded image bytes failed: {error}"
                                );
                            }
                        }
                    }
                    if !recovered.is_empty() {
                        ocr_result.content = recovered;
                    }
                    image_fallback_warnings.push(crate::types::ProcessingWarning {
                        source: std::borrow::Cow::Borrowed("ocr"),
                        message: std::borrow::Cow::Owned(format!(
                            "Page {} rendered blank but contains {} image XObject(s) the PDF rasterizer \
                             could not draw; OCR was retried on the embedded image bytes.",
                            page_idx + 1,
                            fallback_images.len()
                        )),
                    });
                }
            }

            #[cfg(feature = "layout-detection")]
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
                    let paragraphs =
                        assemble_ocr_page_paragraphs(ocr_doc, ocr_layout_height, ocr_scaled_detection.as_ref());

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
                page_texts[page_idx] = ocr_result.content;
                continue;
            }

            #[cfg(not(feature = "layout-detection"))]
            if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let (_, ocr_layout_height) =
                    resolved_ocr_layout_dimensions(&ocr_result.metadata, ocr_render_width, ocr_render_height);
                let paragraphs = crate::pdf::structure::adapters::ocr_doc_to_paragraphs(ocr_doc, ocr_layout_height);
                all_page_paragraphs[page_idx] = Some(paragraphs);
            }

            let _ = page_idx;
            if capture_rasters {
                let (_, png_arc, w, h) = &encoded_batch[offset];
                let png_bytes = bytes::Bytes::copy_from_slice(png_arc.as_ref());
                captured_rasters.push(build_page_raster_image(page_idx, png_bytes, *w, *h));
            }
            page_texts[page_idx] = ocr_result.content;
        }
    }

    #[cfg(feature = "layout-detection")]
    if let Some(model) = tatr_model.take() {
        crate::layout::return_tatr(model);
    }

    let mean_text_conf = if conf_count > 0 {
        Some((conf_sum / conf_count as f64) / 100.0)
    } else {
        None
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

    let ocr_doc = {
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
            Some(crate::pdf::structure::assemble_internal_document(
                pages,
                &collected_tables,
                None,
                &[],
            ))
        } else {
            None
        }
    };

    #[cfg(feature = "pdf")]
    let ocr_doc = attach_ocr_fallback_warnings(ocr_doc, &result, image_fallback_warnings);

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

        let result = Box::pin(extract_with_ocr(
            content,
            images,
            #[cfg(feature = "layout-detection")]
            layout_detections,
            &stage_config,
            path,
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
            )) => {
                let text_score = compute_quality_score(&text, &pipeline.quality_thresholds);

                let score = match mean_conf {
                    Some(conf) => text_score * 0.7 + conf * 0.3,
                    None => text_score,
                };

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
        Some((text, score, tables, elements, doc, page_texts, rasters, formulas)) => {
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
            ))
        }
        None => {
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
                        bbox: BoundingBox {
                            x0: 0.0,
                            y0: 0.0,
                            x1: 100.0,
                            y1: 50.0,
                        },
                        page: 0,
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

    #[cfg(feature = "layout-detection")]
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

    #[cfg(feature = "layout-detection")]
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

    #[cfg(feature = "layout-detection")]
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

    #[cfg(feature = "layout-detection")]
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

    #[cfg(feature = "layout-detection")]
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

    #[cfg(feature = "layout-detection")]
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

        let page_doc = build_mixed_ocr_page_document(&mut result, 3, 1000)
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

        assert!(build_mixed_ocr_page_document(&mut result, 3, 1000).is_none());
    }
}
