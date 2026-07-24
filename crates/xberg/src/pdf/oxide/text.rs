//! PDF text extraction using the pdf_oxide backend.

use super::OxideDocument;
use crate::core::config::{ExtractionConfig, PageConfig};
use crate::pdf::error::{PdfError, Result};
use crate::pdf::metadata::PdfExtractionMetadata;
use crate::pdf::structure::constants::{COALESCE_THRESHOLD, MAX_GLYPH_JITTER_PT, MIN_DISORDER_COUNT};
use crate::pdf::text::{contains_html_markup, fix_pdf_control_chars};
use crate::types::{PageBoundary, PageContent};
use pdf_oxide::document::ReadingOrder;
use std::borrow::Cow;

/// Result type for PDF text extraction with optional page tracking.
type PdfTextExtractionResult = (String, Option<Vec<PageBoundary>>, Option<Vec<PageContent>>);

/// Result type for unified PDF text and metadata extraction.
///
/// Contains text, optional page boundaries, optional per-page content, and metadata.
pub type OxideUnifiedExtractionResult = (
    String,
    Option<Vec<PageBoundary>>,
    Option<Vec<PageContent>>,
    PdfExtractionMetadata,
);

/// Extract text and metadata from a PDF document in a single pass.
///
/// This is the oxide equivalent of `extract_text_and_metadata_from_pdf_document`.
/// It extracts both text and metadata in one pass through the document.
pub(crate) fn extract_text_and_metadata(
    doc: &mut OxideDocument,
    extraction_config: Option<&ExtractionConfig>,
) -> Result<OxideUnifiedExtractionResult> {
    let page_config = extraction_config.and_then(|c| c.pages.as_ref());
    let (text, boundaries, page_contents) = extract_text_from_oxide_document(doc, page_config, extraction_config)?;

    let scanned_min_confidence = extraction_config
        .map(|c| c.ocr_strategy.effective_min_confidence())
        .unwrap_or(crate::core::config::DEFAULT_SCANNED_MIN_CONFIDENCE);
    let ocr_quality_thresholds = extraction_config
        .and_then(|c| c.ocr.as_ref())
        .and_then(|o| o.quality_thresholds.clone())
        .unwrap_or_default();
    let metadata = super::metadata::extract_metadata_from_oxide_document(
        doc,
        boundaries.as_deref(),
        &text,
        scanned_min_confidence,
        &ocr_quality_thresholds,
    )?;

    Ok((text, boundaries, page_contents, metadata))
}

/// Extract text spans with bounding boxes from a single page.
///
/// Returns `(text_spans)` where each span contains the text, x, y, width, and height
/// in PDF coordinate space (points, y=0 at bottom of page).
///
/// This is used by reading-order reconstruction to project spans onto layout regions.
#[cfg(feature = "layout-detection")]
pub(crate) fn extract_spans_from_page(
    doc: &mut pdf_oxide::PdfDocument,
    page_index: usize,
) -> Result<Vec<crate::extractors::pdf::reading_order::TextSpan>> {
    use pdf_oxide::document::ReadingOrder;

    let page_text_data = super::guard_oxide_panic(
        || {
            doc.extract_page_text_with_options(page_index, ReadingOrder::ColumnAware)
                .map_err(|e| PdfError::TextExtractionFailed(format!("Failed to extract page text: {}", e)))
        },
        |panic| PdfError::TextExtractionFailed(format!("Page text extraction panicked in pdf_oxide: {}", panic)),
    )?;

    let spans = page_text_data
        .spans
        .iter()
        .map(|span| crate::extractors::pdf::reading_order::TextSpan {
            text: span.text.clone(),
            x: span.bbox.x,
            y: span.bbox.y,
            width: span.bbox.width,
            height: span.bbox.height,
        })
        .collect();

    Ok(spans)
}

/// Extract text from a pdf_oxide document with optional page boundary tracking.
///
/// Mirrors the signature and behaviour of `extract_text_from_pdf_document`.
///
/// When `page_config` is `Some`, tracks byte offsets and optionally collects
/// per-page `PageContent` entries.
///
/// When `page_config` is `None` but `extraction_config` requires per-page boundaries
/// (i.e. `force_ocr_pages` is set or an `ocr` config is present for quality evaluation),
/// boundary tracking is enabled automatically with a default `PageConfig` so that the
/// mixed-OCR and quality-threshold codepaths receive the offsets they need.
///
/// Otherwise the fast path is used (no per-page tracking).
pub(crate) fn extract_text_from_oxide_document(
    doc: &mut OxideDocument,
    page_config: Option<&PageConfig>,
    extraction_config: Option<&ExtractionConfig>,
) -> Result<PdfTextExtractionResult> {
    let needs_boundaries =
        extraction_config.is_some_and(|c| c.force_ocr_pages.as_ref().is_some_and(|p| !p.is_empty()) || c.ocr.is_some());

    if let Some(config) = page_config {
        extract_text_with_tracking(doc, config)
    } else if needs_boundaries {
        let default_config = PageConfig::default();
        extract_text_with_tracking(doc, &default_config)
    } else {
        extract_text_fast_path(doc)
    }
}

/// Fast path: extract text without page tracking.
///
/// Iterates pages one-by-one, applies control-char fixes and optional HTML
/// conversion, and builds a single concatenated string. Pre-allocates capacity
/// after sampling the first 5 pages.
fn extract_text_fast_path(doc: &mut OxideDocument) -> Result<PdfTextExtractionResult> {
    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::TextExtractionFailed(format!("Failed to get page count: {}", e)))?;

    let mut content = String::new();
    let mut total_sample_size = 0usize;
    let mut sample_count = 0;

    for page_idx in 0..page_count {
        let page_text = extract_page_text_column_aware(&mut doc.doc, page_idx)?;

        let page_size = page_text.len();

        if page_idx > 0 {
            content.push_str("\n\n");
        }

        let cleaned = apply_text_cleanup(&page_text);
        content.push_str(&cleaned);

        if page_idx < 5 {
            total_sample_size += page_size;
            sample_count += 1;
        }

        if page_idx == 4 && sample_count > 0 && page_count > 5 {
            let avg_page_size = total_sample_size / sample_count;
            let estimated_remaining = avg_page_size * (page_count - 5);
            content.reserve(estimated_remaining + (estimated_remaining / 10));
        }
    }

    Ok((content, None, None))
}

/// Extract text with page boundary and content tracking.
///
/// Mirrors `extract_text_lazy_with_tracking`: tracks byte
/// offsets for each page, optionally collects per-page `PageContent`, and inserts
/// page markers when configured.
fn extract_text_with_tracking(doc: &mut OxideDocument, config: &PageConfig) -> Result<PdfTextExtractionResult> {
    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::TextExtractionFailed(format!("Failed to get page count: {}", e)))?;

    let mut content = String::new();
    let mut boundaries = Vec::with_capacity(page_count);
    let mut page_contents = if config.extract_pages {
        Some(Vec::with_capacity(page_count))
    } else {
        None
    };

    let mut total_sample_size = 0usize;
    let mut sample_count = 0;

    for page_idx in 0..page_count {
        let page_number = page_idx + 1;

        let page_text = extract_page_text_column_aware(&mut doc.doc, page_idx)?;

        let page_size = page_text.len();

        if page_idx < 5 {
            total_sample_size += page_size;
            sample_count += 1;
        }

        if config.insert_page_markers {
            let marker = config.marker_format.replace("{page_num}", &page_number.to_string());
            content.push_str(&marker);
        } else if page_idx > 0 {
            content.push_str("\n\n");
        }

        let cleaned = apply_text_cleanup(&page_text);

        let byte_start = content.len();
        content.push_str(&cleaned);
        let byte_end = content.len();

        boundaries.push(PageBoundary {
            byte_start,
            byte_end,
            page_number: page_number as u32,
        });

        if let Some(ref mut pages) = page_contents {
            let is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&cleaned));
            pages.push(PageContent {
                page_number: page_number as u32,
                content: cleaned.into_owned(),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank,
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
            });
        }

        if page_idx == 4 && page_count > 5 && sample_count > 0 {
            let avg_page_size = total_sample_size / sample_count;
            let estimated_remaining = avg_page_size * (page_count - 5);
            let separator_overhead = (page_count - 5) * 3;
            content.reserve(estimated_remaining + separator_overhead + (estimated_remaining / 10));
        }
    }

    Ok((content, Some(boundaries), page_contents))
}

/// Collect Widget annotation field values for the given page, sorted top-to-bottom.
///
/// Returns `(mid_y_pdf, value_text)` pairs. `mid_y_pdf` is the vertical midpoint of
/// the Widget's bounding rectangle in PDF page coordinates (Y=0 at bottom of page,
/// higher values are higher on the page). The list is sorted descending by Y so that
/// entries nearer the top of the page come first, preserving visual reading order when
/// the values are appended to the assembled span text.
///
/// Empty values and annotations without a `/V` entry are excluded. This function is
/// intentionally infallible: a failed `get_annotations` call is logged at DEBUG level
/// and returns an empty list so that the rest of the extraction path is unaffected.
fn collect_widget_field_values(doc: &pdf_oxide::PdfDocument, page_index: usize) -> Vec<(f64, String)> {
    let annotations = match doc.get_annotations(page_index) {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(
                page = page_index,
                "pdf_oxide: could not read annotations for widget values: {e}"
            );
            return Vec::new();
        }
    };

    let mut widgets: Vec<(f64, String)> = annotations
        .into_iter()
        .filter(|a| a.subtype_enum == pdf_oxide::AnnotationSubtype::Widget)
        .filter_map(|a| {
            let value = a.field_value?.trim().to_string();
            if value.is_empty() {
                return None;
            }
            let mid_y = a.rect.map_or(f64::NEG_INFINITY, |r| (r[1] + r[3]) / 2.0);
            Some((mid_y, value))
        })
        .collect();

    widgets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    widgets
}

/// Append Widget form-field values that are absent from `text`.
///
/// Handles interactive (non-flattened) PDFs where field values live only in Widget `/V`
/// entries and are absent from the page content stream. Values already present in `text`
/// (e.g. flattened PDFs where the appearance stream was rendered into the content stream)
/// are skipped to prevent duplication.
///
/// Deduplication uses substring matching: if `value` appears anywhere in `text` the field
/// is skipped. This is intentionally simple — the common case is a verbatim match between
/// the rendered appearance text and the Widget `/V` string. It can produce false negatives
/// when the field value is a substring of surrounding prose (e.g. value "Smith" suppressed
/// when content already contains "John Smith"). This is an acceptable trade-off to avoid
/// duplicating values in flattened PDFs; tighter word-boundary deduplication can be added
/// when evidence of real-world false negatives is available.
///
/// Values are appended after all content-stream text, not interleaved at their bounding-box
/// positions. This is the intended ordering for the initial implementation: interactive
/// PDFs rarely have dense label+value proximity requirements, and span-level interleaving
/// would require re-sorting the column-aware span list which is not guaranteed to be
/// monotonically ordered by Y.
///
/// Appends in top-to-bottom page order (descending by annotation mid-Y).
fn append_missing_widget_values(text: &mut String, widgets: &[(f64, String)]) {
    for (_, value) in widgets {
        if !text.contains(value.as_str()) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(value);
        }
    }
}

/// Returns true when `spans` exhibits the glyph-fragmentation signature (issue #962).
///
/// See `crate::pdf::structure::constants` for the threshold values and their justification.
///
/// pdf_oxide's ColumnAware reading order groups all spans at one y-level before moving
/// to the next. For Word-exported PDFs where each glyph has its own BT…ET block with a
/// sinusoidal y-jitter, this produces groups ordered by y-level rather than by reading
/// order: "et" (y=703) appears before "H" (y=700) even though "H" comes first visually.
///
/// Two-part signature:
/// 1. Both spans are short (≤ 3 chars): per-glyph BT/ET always produces single-character
///    spans; multi-character spans are word-level and cannot be glyph artifacts.
/// 2. The spans are on the same visual line (y-gap ≤ MAX_GLYPH_JITTER_PT when heights
///    are zero, or < half the measured height otherwise) yet the x-coordinate resets
///    significantly leftward — indicating a new y-group started mid-reading-order.
///
/// ≥ MIN_DISORDER_COUNT such events means position-based reconstruction is needed.
fn is_fragmented_span_list(spans: &[pdf_oxide::layout::TextSpan]) -> bool {
    let mut disorder_count = 0;
    for window in spans.windows(2) {
        let prev = &window[0];
        let cur = &window[1];

        if prev.text.chars().count() > 3 || cur.text.chars().count() > 3 {
            continue;
        }

        let y_gap = (prev.bbox.y - cur.bbox.y).abs();

        let eff_height = prev.bbox.height.max(cur.bbox.height);
        let same_line = if eff_height > 0.0 {
            y_gap < eff_height * 0.5
        } else {
            y_gap <= MAX_GLYPH_JITTER_PT
        };

        if same_line && cur.bbox.x < prev.bbox.x - prev.font_size {
            disorder_count += 1;
            if disorder_count >= MIN_DISORDER_COUNT {
                return true;
            }
        }
    }
    false
}

/// Rebuild readable text from a glyph-fragmented span list (issue #962).
///
/// Algorithm:
/// 1. Sort spans by y-descending (top-of-page first in PDF coordinates).
/// 2. Group by chained y-proximity: consecutive spans within COALESCE_THRESHOLD pt
///    of the previous span belong to the same visual line.
/// 3. Within each group sort by x-ascending (left-to-right reading order).
/// 4. Concatenate, inserting a space wherever the x-gap between adjacent spans
///    exceeds font_size * 0.5.
fn rebuild_text_from_fragmented_spans(spans: &[pdf_oxide::layout::TextSpan]) -> String {
    if spans.is_empty() {
        return String::new();
    }

    let mut sorted: Vec<&pdf_oxide::layout::TextSpan> = spans.iter().collect();
    sorted.sort_by(|a, b| b.bbox.y.partial_cmp(&a.bbox.y).unwrap_or(std::cmp::Ordering::Equal));

    let mut groups: Vec<Vec<&pdf_oxide::layout::TextSpan>> = Vec::new();
    for span in sorted {
        let belongs = groups.last().is_some_and(|g| {
            let prev_y = g.last().unwrap().bbox.y;
            (span.bbox.y - prev_y).abs() <= COALESCE_THRESHOLD
        });
        if belongs {
            groups.last_mut().unwrap().push(span);
        } else {
            groups.push(vec![span]);
        }
    }

    let mut result = String::new();
    for (gi, group) in groups.iter_mut().enumerate() {
        group.sort_by(|a, b| a.bbox.x.partial_cmp(&b.bbox.x).unwrap_or(std::cmp::Ordering::Equal));
        if gi > 0 {
            result.push('\n');
        }
        let font_size = group.iter().map(|s| s.font_size).fold(0.0_f32, f32::max);
        let space_threshold = font_size * 0.5;
        let mut prev_end_x = f32::NEG_INFINITY;
        for span in group.iter() {
            if prev_end_x.is_finite() && span.bbox.x - prev_end_x > space_threshold {
                result.push(' ');
            }
            result.push_str(&span.text);
            prev_end_x = span.bbox.x + span.bbox.width;
        }
    }
    result
}

/// Extract text from a single page using column-aware reading order.
///
/// Uses `extract_page_text_with_options` with `ReadingOrder::ColumnAware` to
/// apply XY-Cut column detection. This reads each column top-to-bottom before
/// moving to the next, avoiding interleaved text in multi-column layouts.
///
/// Detects paragraph breaks via vertical gap heuristics: when the gap between
/// lines exceeds 1.5x the median line height, inserts a paragraph break (\n\n).
///
/// Applies a fragmentation repair pass for PDFs that position each glyph via its
/// own BT…ET block (issue #962): detected by ≥ MIN_DISORDER_COUNT same-line x-reset
/// events, which occur when ColumnAware ordering groups spans by y-level rather than
/// reading order; repaired by re-sorting on position rather than relying on stream order.
fn extract_page_text_column_aware(doc: &mut pdf_oxide::PdfDocument, page_index: usize) -> Result<String> {
    let widgets = collect_widget_field_values(doc, page_index);

    let page_text_data = super::guard_oxide_panic(
        || {
            doc.extract_page_text_with_options(page_index, ReadingOrder::ColumnAware)
                .map_err(|e| {
                    PdfError::TextExtractionFailed(format!("Page {} text extraction failed: {}", page_index + 1, e))
                })
        },
        |panic| {
            PdfError::TextExtractionFailed(format!(
                "Page {} text extraction panicked in pdf_oxide: {}",
                page_index + 1,
                panic
            ))
        },
    )?;

    if is_fragmented_span_list(&page_text_data.spans) {
        tracing::debug!(
            span_count = page_text_data.spans.len(),
            "glyph fragmentation detected — rebuilding text from span positions (#962)"
        );
        let mut text = rebuild_text_from_fragmented_spans(&page_text_data.spans);
        append_missing_widget_values(&mut text, &widgets);
        return Ok(text);
    }

    let mut heights: Vec<f32> = page_text_data.spans.iter().map(|s| s.bbox.height).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_height = if heights.is_empty() {
        1.0
    } else {
        heights[heights.len() / 2]
    };
    let paragraph_gap_threshold = median_height * 1.5;

    tracing::debug!(
        span_count = page_text_data.spans.len(),
        median_height,
        paragraph_gap_threshold,
        "paragraph break detection initialized"
    );

    let mut text = String::with_capacity(page_text_data.spans.len() * 20);
    let mut prev_span: Option<&pdf_oxide::layout::TextSpan> = None;

    for span in page_text_data.spans.iter() {
        if let Some(prev) = prev_span {
            let prev_end_x = prev.bbox.x + prev.bbox.width;
            let y_gap = (prev.bbox.y - span.bbox.y).abs();
            let eff_height = span.bbox.height.max(prev.bbox.height).max(span.font_size * 0.5);
            let same_line = y_gap < eff_height * 0.5;

            if same_line {
                let x_gap = span.bbox.x - prev_end_x;
                if x_gap > span.font_size * 0.15 {
                    text.push(' ');
                }
            } else if y_gap > paragraph_gap_threshold {
                text.push_str("\n\n");
            } else {
                text.push('\n');
            }
        }
        text.push_str(&span.text);
        prev_span = Some(span);
    }

    append_missing_widget_values(&mut text, &widgets);

    Ok(text)
}

/// Apply common text cleanup: fix control chars and optionally convert HTML.
///
/// Returns a `Cow` to avoid allocation when the text is already clean.
fn apply_text_cleanup(text: &str) -> Cow<'_, str> {
    let cleaned = fix_pdf_control_chars(text);

    #[cfg(feature = "html")]
    if contains_html_markup(&cleaned) {
        return Cow::Owned(crate::pdf::text::convert_html_page_text(&cleaned));
    }

    #[cfg(not(feature = "html"))]
    let _ = contains_html_markup(&cleaned);

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::geometry::Rect;
    use pdf_oxide::layout::TextSpan;

    fn span(text: &str, x: f32, y: f32, height: f32, font_size: f32) -> TextSpan {
        TextSpan {
            text: text.to_string(),
            bbox: Rect {
                x,
                y,
                width: font_size * 0.6,
                height,
            },
            font_size,
            ..TextSpan::default()
        }
    }

    /// Build a list of N single-char spans that each trigger a same-line x-disorder
    /// event. All at the same y (zero height fallback path), each span's x is
    /// `prev.x - font_size - 1` so cur.x < prev.x - font_size is always true.
    fn disorder_spans(count: usize) -> Vec<TextSpan> {
        let font_size = 12.0_f32;
        let mut spans = Vec::with_capacity(count + 1);
        let mut x = 300.0_f32;
        for _i in 0..=count {
            spans.push(span("A", x, 700.0, 0.0, font_size));
            x = x - font_size - 1.0;
        }
        spans
    }

    #[test]
    fn fragmentation_detected_at_threshold() {
        let spans = disorder_spans(MIN_DISORDER_COUNT);
        assert!(
            is_fragmented_span_list(&spans),
            "should detect fragmentation at exactly MIN_DISORDER_COUNT ({MIN_DISORDER_COUNT}) events"
        );
    }

    #[test]
    fn fragmentation_not_detected_below_threshold() {
        let spans = disorder_spans(MIN_DISORDER_COUNT - 1);
        assert!(
            !is_fragmented_span_list(&spans),
            "must NOT detect fragmentation with {} events (threshold is {MIN_DISORDER_COUNT})",
            MIN_DISORDER_COUNT - 1
        );
    }

    #[test]
    fn long_spans_never_count_toward_disorder() {
        let font_size = 12.0_f32;
        let mut spans = Vec::new();
        let mut x = 500.0_f32;
        for _ in 0..20 {
            spans.push(span("word", x, 700.0, 0.0, font_size));
            x = x - font_size - 1.0;
        }
        assert!(
            !is_fragmented_span_list(&spans),
            "word-level spans (> 3 chars) must never trigger fragmentation detection"
        );
    }

    #[test]
    fn large_y_gap_not_classified_as_same_line() {
        let spans = vec![span("A", 300.0, 700.0, 0.0, 12.0), span("B", 50.0, 686.0, 0.0, 12.0)];
        assert!(
            !is_fragmented_span_list(&spans),
            "14 pt y-gap must not be classified as same-line (MAX_GLYPH_JITTER_PT={MAX_GLYPH_JITTER_PT})"
        );
    }

    #[test]
    fn empty_spans_returns_false() {
        assert!(!is_fragmented_span_list(&[]));
    }

    #[test]
    fn single_span_returns_false() {
        assert!(!is_fragmented_span_list(&[span("A", 100.0, 700.0, 0.0, 12.0)]));
    }
}
