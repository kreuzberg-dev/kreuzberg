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
    let skip_for_non_text = pre_rendered_doc_present
        && total_chars >= thresholds.non_text_min_chars
        && alnum_ws_ratio < thresholds.alnum_ws_ratio_threshold;

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
    #[cfg(test)]
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

    let stats = NativeTextStats::compute(trimmed, thresholds);

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
#[cfg(all(test, any(feature = "ocr", feature = "ocr-pipeline"), feature = "pdf"))]
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
        let (data, width, height) =
            crate::pdf::render::rotate_png_page_if_needed(rendered.data, rendered.width, rendered.height, rotation)?;
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
        let img = image::load_from_memory(&rendered.data).map_err(|e| crate::XbergError::Parsing {
            message: format!("Failed to decode rendered page {}: {}", idx + 1, e),
            source: None,
        })?;
        let rotation = page_rotations.get(idx).copied().unwrap_or(0);
        let img = crate::pdf::render::rotate_dynamic_image(img, rotation);
        images.push((idx, img));
    }

    Ok(images)
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
    Vec<crate::types::LlmUsage>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::Formula>,
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
            Vec::new(),
            None,
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
            Vec::new(),
            None,
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

    let backend = {
        let registry = crate::plugins::registry::get_ocr_backend_registry();
        let registry = registry.read();
        registry.get(&ocr_config_resolved.backend)?
    };

    let batch_size = crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref());

    let capture_rasters = config.images.as_ref().is_some_and(|c| c.include_page_rasters);
    let ocr_config_owned = ocr_config_resolved;
    let total = page_indices.len();
    let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::with_capacity(total);
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut captured_rasters: Vec<crate::types::ExtractedImage> = Vec::new();

    for batch_start in (0..total).step_by(batch_size) {
        let batch_end = (batch_start + batch_size).min(total);
        let page_images =
            render_selected_pages_from_document(&render_doc, &page_rotations, &page_indices[batch_start..batch_end])?;
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
            for (page_idx, data, _w, _h) in &encoded {
                let backend_clone = Arc::clone(&backend);
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
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = (page_idx + 1) as u32;
                    accumulated_formulas.push(formula);
                }
                ocr_results.insert((page_idx + 1) as u32, extraction_result.content);
            }
        }
        #[cfg(any(not(feature = "tokio-runtime"), target_arch = "wasm32"))]
        {
            for (page_idx, data, _w, _h) in &encoded {
                let mut extraction_result = backend.process_image(data.as_slice(), &ocr_config_owned).await?;
                if let Some(usage) = extraction_result.llm_usage.take() {
                    accumulated_llm_usage.extend(usage);
                }
                for mut formula in std::mem::take(&mut extraction_result.formulas) {
                    formula.page = (*page_idx + 1) as u32;
                    accumulated_formulas.push(formula);
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
    let result = apply_ocr_page_replacements(native_text, boundaries, &accepted_replacements);

    Ok((
        result,
        accepted_replacements,
        accumulated_llm_usage,
        if capture_rasters { Some(captured_rasters) } else { None },
        accumulated_formulas,
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
#[cfg(all(test, any(feature = "ocr", feature = "ocr-pipeline")))]
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
    let replacements: std::collections::BTreeMap<u32, &str> = ocr_results
        .iter()
        .filter_map(|(&page, text)| (!text.trim().is_empty()).then_some((page, text.as_str())))
        .collect();
    if replacements.is_empty() {
        return;
    }

    let containers = analyze_container_markers(&doc.elements);
    let anchors = replacement_anchors(&doc.elements, &containers.inferred_pages, &replacements);
    let planned = plan_merged_elements(&doc.elements, &containers, &replacements, &anchors);
    let (rebuilt, old_to_new) = rebuild_planned_elements(planned, doc.elements.len());
    remap_relationships(&mut doc.relationships, &old_to_new, &rebuilt);
    doc.elements = rebuilt;
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
    anchors: &std::collections::BTreeMap<usize, Vec<(u32, &str)>>,
) -> Vec<PlannedOcrElement> {
    use crate::types::internal::ElementKind;

    let mut planned = Vec::with_capacity(elements.len() + replacements.len());
    for (old_index, element) in elements.iter().enumerate() {
        append_ocr_replacements(&mut planned, anchors.get(&old_index));
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
    append_ocr_replacements(&mut planned, anchors.get(&elements.len()));
    planned
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn append_ocr_replacements(planned: &mut Vec<PlannedOcrElement>, replacements: Option<&Vec<(u32, &str)>>) {
    use crate::types::internal::{ElementKind, InternalElement};
    use crate::types::ocr_elements::OcrElementLevel;

    for &(page, text) in replacements.into_iter().flatten() {
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

    #[cfg(feature = "layout-detection")]
    let layout_ocr_config;
    let ocr_config = {
        #[cfg(feature = "layout-detection")]
        {
            let should_inject = layout_detections.is_some() || backend.emits_structured_markdown();
            if should_inject {
                layout_ocr_config = {
                    let mut cfg = ensure_elements_enabled(base_ocr_config);
                    cfg = inject_layout_config_to_backend(&cfg, config);
                    cfg
                };
                &layout_ocr_config
            } else {
                base_ocr_config
            }
        }
        #[cfg(not(feature = "layout-detection"))]
        {
            base_ocr_config
        }
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
        let ocr_elements = result.ocr_elements.unwrap_or_default();
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
    #[cfg(feature = "layout-detection")]
    let mut all_page_paragraphs: Vec<Option<Vec<crate::pdf::structure::types::PdfParagraph>>> = vec![None; total_pages];
    #[allow(unused_mut)]
    let mut collected_tables: Vec<crate::types::Table> = Vec::new();
    let mut all_ocr_elements: Vec<crate::types::OcrElement> = Vec::new();
    let mut accumulated_llm_usage: Vec<crate::types::LlmUsage> = Vec::new();
    let mut accumulated_formulas: Vec<crate::types::Formula> = Vec::new();
    let mut conf_sum: f64 = 0.0;
    let mut conf_count: usize = 0;

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
                all_ocr_elements.extend(elems.iter().cloned());
            }

            for mut formula in ocr_result.formulas {
                formula.page = (page_idx + 1) as u32;
                accumulated_formulas.push(formula);
            }

            #[cfg(feature = "layout-detection")]
            if let Some(detections) = layout_detections
                && let Some(ref elements) = ocr_result.ocr_elements
                && !elements.is_empty()
            {
                let detection = detections.get(page_idx);

                let ocr_render_width = encoded_batch[offset].2;
                let ocr_render_height = encoded_batch[offset].3;
                let scaled_detection: Option<crate::layout::DetectionResult> = detection.map(|det| {
                    let sx = ocr_render_width as f32 / det.page_width as f32;
                    let sy = ocr_render_height as f32 / det.page_height as f32;
                    let mut scaled = det.clone();
                    scaled.page_width = ocr_render_width;
                    scaled.page_height = ocr_render_height;
                    for region in &mut scaled.detections {
                        region.bbox.x1 *= sx;
                        region.bbox.y1 *= sy;
                        region.bbox.x2 *= sx;
                        region.bbox.y2 *= sy;
                    }
                    scaled
                });

                let recognized_tables = match (scaled_detection.as_ref(), tatr_model.as_mut()) {
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
                        crate::ocr::layout_assembly::recognize_page_tables(&rgb, scaled_det, elements, model)
                    }
                    _ => Vec::new(),
                };

                for rt in &recognized_tables {
                    if !rt.markdown.is_empty() {
                        collected_tables.push(crate::types::Table {
                            cells: rt.cells.clone(),
                            markdown: rt.markdown.clone(),
                            page_number: (page_idx + 1) as u32,
                            bounding_box: None,
                            ..Default::default()
                        });
                    }
                }

                if let Some(ref ocr_doc) = ocr_result.ocr_internal_document {
                    let mut paragraphs =
                        crate::pdf::structure::adapters::ocr_doc_to_paragraphs(ocr_doc, ocr_render_height);

                    if let Some(ref scaled_det) = scaled_detection {
                        let hints = super::layout_hints::detection_to_layout_hints_pixel_space(
                            scaled_det,
                            ocr_render_height as f32,
                        );
                        crate::pdf::structure::layout_classify::apply_layout_overrides(
                            &mut paragraphs,
                            &hints,
                            0.5,
                            0.2,
                            None,
                        );
                    }

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

    #[cfg(feature = "layout-detection")]
    let ocr_doc = {
        let has_structured = all_page_paragraphs.iter().any(|p| p.is_some());
        if has_structured {
            let pages: Vec<Vec<crate::pdf::structure::types::PdfParagraph>> = all_page_paragraphs
                .into_iter()
                .map(|opt| opt.unwrap_or_default())
                .collect();
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
    #[cfg(not(feature = "layout-detection"))]
    let ocr_doc: Option<crate::types::internal::InternalDocument> = {
        let mut doc = crate::types::internal::InternalDocument::new("pdf");
        for paragraph in result.split("\n\n") {
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                doc.push_element(crate::types::internal::InternalElement::text(
                    crate::types::internal::ElementKind::Paragraph,
                    trimmed,
                    0,
                ));
            }
        }
        doc.tables = collected_tables.clone();
        Some(doc)
    };

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
/// Run a multi-backend OCR pipeline with quality-based fallback.
///
/// Images and layout detections are computed once and shared across all stages.
/// Each stage produces OCR output that is scored; if the score meets the
/// pipeline's quality threshold, the result is accepted. Otherwise, the next
/// backend is tried. Returns the best result seen across all stages.
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

    let mut stages = pipeline.stages.clone();
    stages.sort_by_key(|b| std::cmp::Reverse(b.priority));

    let requested_backends: Vec<String> = stages.iter().map(|s| s.backend.clone()).collect();
    let available_stages: Vec<_> = {
        let registry = get_ocr_backend_registry();
        let registry = registry.read();
        stages
            .into_iter()
            .filter(|s| registry.get(&s.backend).is_ok())
            .collect()
    };

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

                match best_result {
                    Some((_, best_score, _, _, _, _, _, _)) if score > best_score => {
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
                    None => {
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
                    _ => {}
                }
            }
            Err(e) => {
                tracing::warn!(
                    backend = %stage.backend,
                    error = %e,
                    "Pipeline: backend failed, trying next"
                );
            }
        }
    }

    match best_result {
        Some((text, score, tables, elements, doc, page_texts, rasters, formulas)) => {
            let threshold = pipeline.quality_thresholds.pipeline_min_quality;
            tracing::warn!(
                score,
                threshold,
                "All OCR pipeline backends produced suboptimal quality, using best result"
            );
            let mut doc = doc.unwrap_or_else(|| {
                let mut d = crate::types::internal::InternalDocument::new("pdf");
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
                     (best score {score:.3} < {threshold:.3}); returning the best-effort result, \
                     which may be inaccurate or incomplete."
                )),
            });
            Ok((
                text,
                tables,
                elements,
                Some(doc),
                accumulated_usage,
                page_texts,
                rasters,
                formulas,
            ))
        }
        None => Err(crate::XbergError::Parsing {
            message: "All OCR pipeline backends failed".to_string(),
            source: None,
        }),
    }
}

/// Clone an OCR config with `include_elements` forced to true.
///
/// Layout assembly requires OCR elements with bounding geometry. This ensures
/// the backend produces them regardless of the user's original config.
#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn ensure_elements_enabled(config: &crate::core::config::ocr::OcrConfig) -> crate::core::config::ocr::OcrConfig {
    let mut config = config.clone();
    match config.element_config.as_mut() {
        Some(ec) => ec.include_elements = true,
        None => {
            config.element_config = Some(crate::types::OcrElementConfig {
                include_elements: true,
                ..Default::default()
            });
        }
    }
    config
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
        assert!(result.3.is_none());
        assert!(result.4.is_empty());
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

        let mut pages: Vec<u32> = formulas.iter().map(|f| f.page).collect();
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
}
