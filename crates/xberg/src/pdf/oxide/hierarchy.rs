//! Font metrics extraction for heading hierarchy detection using the pdf_oxide backend.
//!
//! Uses pdf_oxide's span extraction to get font_size, font_weight, is_italic,
//! and font_name, converting them to `SegmentData` for the backend-agnostic
//! clustering pipeline that assigns heading levels (H1-H6) to text blocks.
//!
//! When the PDF is a tagged PDF with a reliable structure tree, heading roles
//! (H1-H6) are read directly from the tree and assigned via `SegmentData::assigned_role`,
//! bypassing font-size clustering entirely for more accurate heading detection.

use std::collections::HashMap;

use super::OxideDocument;
// Inline-script rejoining measures baselines and character origins in raw page
// coordinates, so it needs the strict page-axis predicate, not the
// rotation-agnostic writing-mode one.
use super::span_geometry::is_horizontal_ltr;
use crate::pdf::error::Result;
use crate::pdf::hierarchy::SegmentData;

const COLUMN_BRIDGE_FRACTION: f32 = 0.6;
const MIN_COLUMN_GUTTER_PTS: f32 = 8.0;
const MIN_COLUMN_SIDE_SPANS: usize = 2;
const MIN_TWO_COLUMN_CONTENT_WIDTH_PTS: f32 = 144.0;
const MIN_PROSE_LINES_PER_SIDE: usize = 4;
const MIN_PROSE_LINE_ALPHA_CHARS: usize = 8;
const MIN_PROSE_LINE_WORDS: usize = 3;
const MIN_PROSE_ALPHA_RATIO: f32 = 0.55;
const MIN_SIDE_BALANCE_RATIO: f32 = 0.15;
const MIN_VERTICAL_OVERLAP_RATIO: f32 = 0.35;
const PROSE_LINE_Y_TOLERANCE_PTS: f32 = 4.0;
const INLINE_SCRIPT_LOOKBACK: usize = 8;
const INLINE_SCRIPT_MIN_FONT_RATIO: f32 = 0.5;
const INLINE_SCRIPT_MAX_FONT_RATIO: f32 = 0.8;
const INLINE_SCRIPT_MIN_BASELINE_SHIFT_EM: f32 = 0.08;
const INLINE_SCRIPT_MAX_BASELINE_SHIFT_EM: f32 = 0.35;
const INLINE_SCRIPT_MAX_SUFFIX_GAP_EM: f32 = 0.12;
const INLINE_SCRIPT_SAME_BASELINE_TOLERANCE_EM: f32 = 0.02;
const INLINE_SCRIPT_MAX_CHARS: usize = 4;
const INLINE_SCRIPT_MIN_WIDTH_COVERAGE: f32 = 0.7;
const INLINE_SCRIPT_MAX_WIDTH_COVERAGE: f32 = 1.3;

#[derive(Debug)]
struct SideSupport {
    prose_line_ys: Vec<f32>,
}

#[derive(Debug)]
struct ScriptAttachment {
    script_index: usize,
    insertion_index: usize,
}

fn is_usable_span(span: &pdf_oxide::layout::TextSpan) -> bool {
    span.artifact_type.is_none()
        && !span.text.trim().is_empty()
        && span.bbox.x.is_finite()
        && span.bbox.y.is_finite()
        && span.bbox.width.is_finite()
        && span.bbox.height.is_finite()
        && span.bbox.width > 0.0
        && span.bbox.height > 0.0
}

fn content_bounds(spans: &[&pdf_oxide::layout::TextSpan]) -> Option<(f32, f32)> {
    let min = spans.iter().map(|span| span.bbox.x).fold(f32::INFINITY, f32::min);
    let max = spans
        .iter()
        .map(|span| span.bbox.x + span.bbox.width)
        .fold(f32::NEG_INFINITY, f32::max);
    (min.is_finite() && max.is_finite() && max > min).then_some((min, max))
}

fn detect_gutter_x(spans: &[&pdf_oxide::layout::TextSpan]) -> Option<f32> {
    if spans.len() < MIN_COLUMN_SIDE_SPANS * 2 {
        return None;
    }
    let (content_min, content_max) = content_bounds(spans)?;
    let content_width = content_max - content_min;
    if content_width < MIN_TWO_COLUMN_CONTENT_WIDTH_PTS {
        return None;
    }

    let bridge_width = content_width * COLUMN_BRIDGE_FRACTION;
    let mut extents: Vec<(f32, f32)> = spans
        .iter()
        .filter(|span| span.bbox.width <= bridge_width)
        .map(|span| (span.bbox.x, span.bbox.x + span.bbox.width))
        .collect();
    if extents.len() < MIN_COLUMN_SIDE_SPANS * 2 {
        return None;
    }
    extents.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut cover_right = extents[0].1;
    let mut best_gap = 0.0_f32;
    let mut best_mid = 0.0_f32;
    let mut left_count = 0usize;
    for (index, extent) in extents.iter().enumerate().skip(1) {
        let gap = extent.0 - cover_right;
        if gap > best_gap {
            best_gap = gap;
            best_mid = (cover_right + extent.0) * 0.5;
            left_count = index;
        }
        cover_right = cover_right.max(extent.1);
    }

    let right_count = extents.len() - left_count;
    let relative_mid = (best_mid - content_min) / content_width;
    (best_gap >= MIN_COLUMN_GUTTER_PTS
        && (0.3..=0.7).contains(&relative_mid)
        && left_count >= MIN_COLUMN_SIDE_SPANS
        && right_count >= MIN_COLUMN_SIDE_SPANS)
        .then_some(best_mid)
}

fn prose_like(text: &str, monospace_spans: usize, span_count: usize) -> bool {
    if monospace_spans * 2 >= span_count.max(1) {
        return false;
    }
    let alpha_chars = text.chars().filter(|ch| ch.is_alphabetic()).count();
    let alphanumeric_chars = text.chars().filter(|ch| ch.is_alphanumeric()).count();
    let words = text
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphabetic))
        .count();
    alpha_chars >= MIN_PROSE_LINE_ALPHA_CHARS
        && words >= MIN_PROSE_LINE_WORDS
        && alpha_chars as f32 / alphanumeric_chars.max(1) as f32 >= MIN_PROSE_ALPHA_RATIO
}

fn side_support(spans: Vec<&pdf_oxide::layout::TextSpan>) -> SideSupport {
    let mut prose_line_ys: Vec<_> = spans
        .into_iter()
        .filter(|span| prose_like(&span.text, usize::from(span.is_monospace), 1))
        .map(|span| span.bbox.y)
        .collect();
    prose_line_ys.sort_by(f32::total_cmp);
    prose_line_ys.dedup_by(|left, right| (*left - *right).abs() <= PROSE_LINE_Y_TOLERANCE_PTS);
    SideSupport { prose_line_ys }
}

fn has_balanced_vertical_support(left: &SideSupport, right: &SideSupport) -> bool {
    let left_count = left.prose_line_ys.len();
    let right_count = right.prose_line_ys.len();
    if left_count < MIN_PROSE_LINES_PER_SIDE || right_count < MIN_PROSE_LINES_PER_SIDE {
        return false;
    }
    let balance = left_count.min(right_count) as f32 / left_count.max(right_count) as f32;
    let extent = |ys: &[f32]| {
        let low = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let high = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (low, high)
    };
    let (left_low, left_high) = extent(&left.prose_line_ys);
    let (right_low, right_high) = extent(&right.prose_line_ys);
    let overlap = (left_high.min(right_high) - left_low.max(right_low)).max(0.0);
    let shorter_extent = (left_high - left_low).min(right_high - right_low);
    balance >= MIN_SIDE_BALANCE_RATIO && shorter_extent > 0.0 && overlap / shorter_extent >= MIN_VERTICAL_OVERLAP_RATIO
}

fn select_reading_order(
    spans: &[pdf_oxide::layout::TextSpan],
    page_width: f32,
    page_height: f32,
) -> pdf_oxide::document::ReadingOrder {
    use pdf_oxide::document::ReadingOrder;

    if !page_width.is_finite() || page_width <= 0.0 || !page_height.is_finite() || page_height <= 0.0 {
        return ReadingOrder::TopToBottom;
    }
    let usable: Vec<_> = spans.iter().filter(|span| is_usable_span(span)).collect();
    let Some((content_min, content_max)) = content_bounds(&usable) else {
        return ReadingOrder::TopToBottom;
    };
    let content_width = content_max - content_min;
    if content_width < MIN_TWO_COLUMN_CONTENT_WIDTH_PTS {
        return ReadingOrder::TopToBottom;
    }
    let body: Vec<_> = usable
        .into_iter()
        .filter(|span| span.bbox.width <= content_width * COLUMN_BRIDGE_FRACTION)
        .collect();
    let gutter_x = detect_gutter_x(&body).unwrap_or((content_min + content_max) * 0.5);
    let left = side_support(
        body.iter()
            .copied()
            .filter(|span| span.bbox.x + span.bbox.width <= gutter_x)
            .collect(),
    );
    let right = side_support(body.iter().copied().filter(|span| span.bbox.x >= gutter_x).collect());
    if has_balanced_vertical_support(&left, &right) {
        ReadingOrder::ColumnAware
    } else {
        ReadingOrder::TopToBottom
    }
}

/// Reorder a page's spans into reading order for the element-based
/// extraction path (GH#1397).
///
/// Applies the guarded sparse-column repair unconditionally, then attempts
/// the dense two-column band-split repair
/// (`super::text::reorder_dense_two_column_page`). That repair carries its
/// own conservative gates (minimum content width, per-line gutter agreement,
/// a minimum span count per column per band, and a prose/table region gate —
/// see its doc comment), so once it reports success the page's reading order
/// is already correct. pdf_oxide's own `ColumnAware` XY-Cut pass is then
/// skipped entirely: XY-Cut re-orders the whole span list from scratch and
/// would otherwise silently discard the band order just applied. This
/// matters because XY-Cut's own valley detection can miss exactly the narrow
/// gutter the dense repair's per-line detector picks up (XY-Cut's default
/// `min_valley_width` is wider than the dense repair's own gutter floor),
/// which is why GH#1397 saw identical output whether `ColumnAware` or
/// `TopToBottom` was requested — XY-Cut was never actually splitting the
/// page. When the dense repair does not apply (single-column page,
/// table-shaped columns, an undetectable gutter, etc.) the previous
/// heuristic-selected XY-Cut fallback runs exactly as before, so non-dense
/// pages are unaffected.
fn reorder_page_reading_order(
    spans: &mut Vec<pdf_oxide::layout::TextSpan>,
    page_width: f32,
    page_height: f32,
    page_index: usize,
) {
    super::text::reorder_sparse_two_column_page(spans, page_width);
    if super::text::reorder_dense_two_column_page(spans, page_width) {
        return;
    }
    apply_xy_cut_if_column_aware(spans, page_width, page_height, page_index);
}

/// Run pdf_oxide's `ColumnAware` XY-Cut pass when `select_reading_order`
/// judges the page a balanced two-column prose layout.
///
/// A failed XY-Cut pass is logged and leaves `spans` in top-to-bottom order
/// (the pre-existing fallback behavior).
fn apply_xy_cut_if_column_aware(
    spans: &mut Vec<pdf_oxide::layout::TextSpan>,
    page_width: f32,
    page_height: f32,
    page_index: usize,
) {
    use pdf_oxide::pipeline::{ReadingOrderContext, ReadingOrderStrategy, XYCutStrategy};

    let order = select_reading_order(spans.as_slice(), page_width, page_height);
    if order != pdf_oxide::document::ReadingOrder::ColumnAware {
        return;
    }
    let context = ReadingOrderContext::new().with_page(page_index as u32);
    match XYCutStrategy::new().apply(spans.clone(), &context) {
        Ok(ordered) => *spans = ordered.into_iter().map(|item| item.span).collect(),
        Err(error) => tracing::debug!(
            page = page_index,
            "pdf_oxide column-aware hierarchy ordering failed; retaining top-to-bottom order: {error}"
        ),
    }
}

fn rejoin_inline_scripts(spans: Vec<pdf_oxide::layout::TextSpan>) -> Vec<pdf_oxide::layout::TextSpan> {
    let mut by_base: HashMap<usize, Vec<ScriptAttachment>> = HashMap::new();
    let mut attached = vec![false; spans.len()];
    for script_index in 0..spans.len() {
        if by_base.contains_key(&script_index) {
            continue;
        }
        let Some((base_index, insertion_index)) = find_inline_script_base(&spans, &attached, script_index) else {
            continue;
        };
        attached[script_index] = true;
        by_base.entry(base_index).or_default().push(ScriptAttachment {
            script_index,
            insertion_index,
        });
    }

    if by_base.is_empty() {
        return spans;
    }

    let mut repaired = Vec::with_capacity(spans.len());
    for (index, span) in spans.iter().enumerate() {
        if attached[index] {
            continue;
        }
        match by_base.remove(&index) {
            Some(scripts) => emit_base_with_scripts(span, scripts, &spans, &mut repaired),
            None => repaired.push(span.clone()),
        }
    }
    repaired
}

fn find_inline_script_base(
    spans: &[pdf_oxide::layout::TextSpan],
    attached: &[bool],
    script_index: usize,
) -> Option<(usize, usize)> {
    let script = spans.get(script_index)?;
    if !is_compact_horizontal_ascii_span(script) {
        return None;
    }

    let start = script_index.saturating_sub(INLINE_SCRIPT_LOOKBACK);
    (start..script_index)
        .filter(|base_index| !attached[*base_index])
        .filter_map(|base_index| {
            let base = &spans[base_index];
            inline_script_insertion(base, script, base_index + 1 == script_index).map(|insertion| {
                (
                    base_index,
                    insertion,
                    (script.bbox.y - base.bbox.y).abs(),
                    horizontal_attachment_distance(base, script),
                    script_index - base_index,
                )
            })
        })
        .min_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| left.3.total_cmp(&right.3))
                .then_with(|| left.4.cmp(&right.4))
        })
        .map(|(base_index, insertion_index, _, _, _)| (base_index, insertion_index))
}

fn inline_script_insertion(
    base: &pdf_oxide::layout::TextSpan,
    script: &pdf_oxide::layout::TextSpan,
    immediately_follows: bool,
) -> Option<usize> {
    if base.artifact_type.is_some()
        || script.artifact_type.is_some()
        || !is_horizontal_ltr(base)
        || !base.text.is_ascii()
        || !base.text.chars().any(|character| character.is_ascii_alphabetic())
        || !has_valid_span_geometry(base)
        || !has_valid_span_geometry(script)
    {
        return None;
    }
    let font_ratio = script.font_size / base.font_size;
    if !(INLINE_SCRIPT_MIN_FONT_RATIO..=INLINE_SCRIPT_MAX_FONT_RATIO).contains(&font_ratio) {
        return None;
    }

    let base_right = base.bbox.x + base.bbox.width;
    let gap = script.bbox.x - base_right;
    let baseline_shift = (script.bbox.y - base.bbox.y).abs();
    if baseline_shift > base.font_size * INLINE_SCRIPT_MAX_BASELINE_SHIFT_EM {
        return None;
    }
    let same_baseline_suffix = immediately_follows
        && gap >= 0.0
        && gap <= base.font_size * INLINE_SCRIPT_MAX_SUFFIX_GAP_EM
        && baseline_shift <= base.font_size * INLINE_SCRIPT_SAME_BASELINE_TOLERANCE_EM;
    let shifted_script = baseline_shift >= base.font_size * INLINE_SCRIPT_MIN_BASELINE_SHIFT_EM
        && baseline_shift <= base.font_size * INLINE_SCRIPT_MAX_BASELINE_SHIFT_EM;
    let normalized_rise = script.text_rise.abs() * script.font_size / base.font_size;
    let explicit_rise = normalized_rise.is_finite()
        && (INLINE_SCRIPT_MIN_BASELINE_SHIFT_EM..=INLINE_SCRIPT_MAX_BASELINE_SHIFT_EM).contains(&normalized_rise);
    if !same_baseline_suffix && !shifted_script && !explicit_rise {
        return None;
    }
    if script.bbox.x < base.bbox.x || gap > base.font_size * INLINE_SCRIPT_MAX_SUFFIX_GAP_EM {
        return None;
    }

    let char_count = base.text.chars().count();
    if script.bbox.x >= base_right {
        return Some(char_count);
    }
    character_origins(base).map(|origins| origins.partition_point(|origin| *origin < script.bbox.x))
}

fn is_compact_horizontal_ascii_span(span: &pdf_oxide::layout::TextSpan) -> bool {
    let char_count = span.text.chars().count();
    char_count > 0
        && char_count <= INLINE_SCRIPT_MAX_CHARS
        && span.artifact_type.is_none()
        && span.text.is_ascii()
        && !span.text.chars().any(char::is_whitespace)
        && span.text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '=' | '(' | ')' | ',' | '.')
        })
        && is_horizontal_ltr(span)
}

fn has_valid_span_geometry(span: &pdf_oxide::layout::TextSpan) -> bool {
    span.bbox.x.is_finite()
        && span.bbox.y.is_finite()
        && span.bbox.width.is_finite()
        && span.bbox.height.is_finite()
        && span.font_size.is_finite()
        && span.bbox.width > 0.0
        && span.bbox.height > 0.0
        && span.font_size > 0.0
}

fn character_origins(span: &pdf_oxide::layout::TextSpan) -> Option<Vec<f32>> {
    let char_count = span.text.chars().count();
    let bbox_right = span.bbox.x + span.bbox.width;
    if span.char_x_offsets.len() == char_count
        && span.char_x_offsets.iter().all(|origin| origin.is_finite())
        && span.char_x_offsets.windows(2).all(|pair| pair[0] < pair[1])
        && span.char_x_offsets.first().is_some_and(|origin| *origin >= span.bbox.x)
        && span.char_x_offsets.last().is_some_and(|origin| *origin <= bbox_right)
    {
        return Some(span.char_x_offsets.clone());
    }

    let width_sum: f32 = span.char_widths.iter().sum();
    let coverage = width_sum / span.bbox.width;
    if span.char_widths.len() != char_count
        || !span.char_widths.iter().all(|width| width.is_finite() && *width > 0.0)
        || !width_sum.is_finite()
        || width_sum <= 0.0
        || !(INLINE_SCRIPT_MIN_WIDTH_COVERAGE..=INLINE_SCRIPT_MAX_WIDTH_COVERAGE).contains(&coverage)
    {
        return None;
    }

    let scale = span.bbox.width / width_sum;
    let mut x = span.bbox.x;
    Some(
        span.char_widths
            .iter()
            .map(|width| {
                let origin = x;
                x += width * scale;
                origin
            })
            .collect(),
    )
}

fn horizontal_attachment_distance(base: &pdf_oxide::layout::TextSpan, script: &pdf_oxide::layout::TextSpan) -> f32 {
    let base_right = base.bbox.x + base.bbox.width;
    if script.bbox.x <= base_right {
        0.0
    } else {
        script.bbox.x - base_right
    }
}

fn emit_base_with_scripts(
    base: &pdf_oxide::layout::TextSpan,
    mut scripts: Vec<ScriptAttachment>,
    spans: &[pdf_oxide::layout::TextSpan],
    output: &mut Vec<pdf_oxide::layout::TextSpan>,
) {
    scripts.sort_by(|left, right| {
        left.insertion_index.cmp(&right.insertion_index).then_with(|| {
            spans[left.script_index]
                .bbox
                .x
                .total_cmp(&spans[right.script_index].bbox.x)
        })
    });
    let mut range_start = 0;
    let char_count = base.text.chars().count();
    for script in scripts {
        let fragment = (script.insertion_index > range_start)
            .then(|| split_span(base, range_start, script.insertion_index))
            .flatten();
        let normalized = normalize_script_span(&spans[script.script_index], base);
        if script.insertion_index == char_count {
            if let Some(mut fragment) = fragment {
                append_span_text(&mut fragment, &normalized);
                output.push(fragment);
            } else if let Some(previous) = output.last_mut() {
                append_span_text(previous, &normalized);
            }
        } else {
            output.extend(fragment);
            output.push(normalized);
        }
        range_start = script.insertion_index;
    }
    if let Some(fragment) = split_span(base, range_start, char_count) {
        output.push(fragment);
    }
}

fn append_span_text(target: &mut pdf_oxide::layout::TextSpan, suffix: &pdf_oxide::layout::TextSpan) {
    target.text.push_str(&suffix.text);
    let target_right = target.bbox.x + target.bbox.width;
    let suffix_right = suffix.bbox.x + suffix.bbox.width;
    target.bbox.width = target_right.max(suffix_right) - target.bbox.x;
    target.char_x_offsets.clear();
    target.char_widths.clear();
}

fn split_span(span: &pdf_oxide::layout::TextSpan, start: usize, end: usize) -> Option<pdf_oxide::layout::TextSpan> {
    if start >= end {
        return None;
    }
    let chars: Vec<char> = span.text.chars().collect();
    if start == 0 && end == chars.len() {
        return Some(span.clone());
    }
    let origins = character_origins(span)?;
    let mut fragment = span.clone();
    fragment.text = chars[start..end].iter().collect();
    fragment.bbox.x = origins[start];
    let end_x = origins.get(end).copied().unwrap_or(span.bbox.x + span.bbox.width);
    fragment.bbox.width = (end_x - fragment.bbox.x).max(0.0);
    fragment.char_x_offsets = origins[start..end].to_vec();
    if span.char_widths.len() == chars.len() {
        fragment.char_widths = span.char_widths[start..end].to_vec();
    } else {
        fragment.char_widths.clear();
    }
    Some(fragment)
}

fn normalize_script_span(
    script: &pdf_oxide::layout::TextSpan,
    base: &pdf_oxide::layout::TextSpan,
) -> pdf_oxide::layout::TextSpan {
    let mut normalized = script.clone();
    normalized.bbox.y = base.bbox.y;
    normalized.bbox.height = base.bbox.height;
    normalized.font_name.clone_from(&base.font_name);
    normalized.font_size = base.font_size;
    normalized.font_weight = base.font_weight;
    normalized.is_italic = base.is_italic;
    normalized.is_monospace = base.is_monospace;
    normalized.mcid = base.mcid;
    normalized.mcid_scope.clone_from(&base.mcid_scope);
    normalized.heading_level = base.heading_level;
    normalized.text_rise = 0.0;
    normalized
}

/// Extract text segments with font metrics from a PDF page using pdf_oxide.
///
/// Returns `SegmentData` objects containing text, position, and font metadata
/// (size, bold, italic, monospace). These feed into the existing backend-agnostic
/// font size clustering pipeline for heading detection.
///
/// Starts with top-to-bottom reading order and switches to column-aware ordering
/// only when the page has conservative geometric evidence of two prose columns.
///
/// # Arguments
///
/// * `doc` - Mutable reference to the oxide document
/// * `page_index` - Zero-based page index
///
/// # Returns
///
/// Vector of `SegmentData` objects with font metrics for hierarchy detection.
pub(crate) fn extract_segments_from_page(doc: &mut OxideDocument, page_index: usize) -> Result<Vec<SegmentData>> {
    extract_segments_from_page_inner(doc, page_index, &HashMap::new())
}

/// Inner implementation of per-page segment extraction.
///
/// When `mcid_roles` is non-empty, spans with matching MCIDs receive pre-assigned
/// heading levels from the PDF structure tree.
fn extract_segments_from_page_inner(
    doc: &mut OxideDocument,
    page_index: usize,
    mcid_roles: &HashMap<u32, Option<u8>>,
) -> Result<Vec<SegmentData>> {
    let mut page_text_data = match doc
        .doc
        .extract_page_text_with_options(page_index, pdf_oxide::document::ReadingOrder::TopToBottom)
    {
        Ok(data) => data,
        Err(e) => {
            tracing::debug!(
                page = page_index,
                "pdf_oxide extract_page_text_with_options failed for hierarchy: {e}"
            );
            return Ok(Vec::new());
        }
    };
    reorder_page_reading_order(
        &mut page_text_data.spans,
        page_text_data.page_width,
        page_text_data.page_height,
        page_index,
    );
    let spans = rejoin_inline_scripts(page_text_data.spans);

    let segments: Vec<SegmentData> = spans
        .into_iter()
        .filter(|span| {
            if span.artifact_type.is_some() {
                return false;
            }
            !span.text.trim().is_empty()
        })
        .map(|span| {
            let is_bold = span.font_weight == pdf_oxide::layout::text_block::FontWeight::Bold;
            let bbox = &span.bbox;

            let pdf_baseline_y = bbox.y;
            let pdf_y = bbox.y;

            let assigned_role = span.mcid.and_then(|mcid| mcid_roles.get(&mcid).copied()).flatten();

            SegmentData {
                text: span.text,
                x: bbox.x,
                y: pdf_y,
                width: bbox.width,
                height: bbox.height,
                font_size: span.font_size,
                is_bold,
                is_italic: span.is_italic,
                is_monospace: span.is_monospace,
                baseline_y: pdf_baseline_y,
                assigned_role,
            }
        })
        .collect();

    Ok(dedupe_redrawn_segments(segments))
}

/// Minimum positional tolerance (pt) for treating two identical-text spans as
/// one re-drawn glyph run (covers sub-point faux-bold offsets even on tiny text).
const REDRAWN_MIN_TOLERANCE_PTS: f32 = 1.0;

/// How many previously kept segments to compare against. Re-drawn duplicates are
/// emitted adjacently (same show-text operation repeated), so a short window is
/// sufficient and keeps the pass linear.
const REDRAWN_LOOKBACK: usize = 8;

/// Collapse re-drawn text spans: identical text at overlapping positions.
///
/// PDFs simulate bold by drawing the same run twice with a small offset, and some
/// generators re-draw runs with different font attributes overlaid. Keeping both
/// copies duplicates output text and fuses lines so heading classification fails
/// (issue-1114 fixture). The tolerance is relative to the span's own extent —
/// duplicates must substantially overlap — so identical short strings in adjacent
/// table cells or rows are never collapsed. The kept segment absorbs the
/// bold/italic signal of its duplicates because a double-draw is precisely a
/// boldness cue.
fn dedupe_redrawn_segments(segments: Vec<SegmentData>) -> Vec<SegmentData> {
    let mut kept: Vec<SegmentData> = Vec::with_capacity(segments.len());
    for seg in segments {
        let window_start = kept.len().saturating_sub(REDRAWN_LOOKBACK);
        if let Some(prev) = kept[window_start..].iter_mut().find(|prev| {
            let dx_tol = (prev.width.min(seg.width) * 0.5).max(REDRAWN_MIN_TOLERANCE_PTS);
            let dy_tol = (prev.height.min(seg.height) * 0.5).max(REDRAWN_MIN_TOLERANCE_PTS);
            prev.text == seg.text && (prev.x - seg.x).abs() <= dx_tol && (prev.y - seg.y).abs() <= dy_tol
        }) {
            prev.is_bold |= seg.is_bold;
            prev.is_italic |= seg.is_italic;
            if seg.font_size > prev.font_size {
                prev.font_size = seg.font_size;
            }
            continue;
        }
        kept.push(seg);
    }
    kept
}

/// Try to extract segments using the PDF structure tree for heading detection.
///
/// Checks `MarkInfo` to see if the structure tree is reliable (marked && !suspects),
/// then traverses the tree to build MCID → heading-level mappings per page.
/// Spans are then extracted normally but annotated with `assigned_role` from the tree.
///
/// Returns `(segments, used_structure_tree)`. When `used_structure_tree` is true,
/// the caller should skip font-size clustering and use the pre-assigned roles.
fn extract_segments_with_structure_tree(doc: &mut OxideDocument) -> Result<(Vec<Vec<SegmentData>>, bool)> {
    let mark_info = match doc.doc.mark_info() {
        Ok(mi) => mi,
        Err(e) => {
            tracing::debug!("pdf_oxide: mark_info() failed, skipping structure tree: {e}");
            return Ok((Vec::new(), false));
        }
    };

    if !mark_info.is_structure_reliable() {
        tracing::debug!(
            marked = mark_info.marked,
            suspects = mark_info.suspects,
            "pdf_oxide: structure tree not reliable, falling back to font-size clustering"
        );
        return Ok((Vec::new(), false));
    }

    let struct_tree = match doc.doc.structure_tree() {
        Ok(Some(tree)) => tree,
        Ok(None) => {
            tracing::debug!("pdf_oxide: no structure tree found despite marked=true");
            return Ok((Vec::new(), false));
        }
        Err(e) => {
            tracing::debug!("pdf_oxide: structure_tree() failed: {e}");
            return Ok((Vec::new(), false));
        }
    };

    let all_page_content = pdf_oxide::structure::traverse_structure_tree_all_pages(&struct_tree);

    let heading_count: usize = all_page_content
        .values()
        .flat_map(|contents| contents.iter())
        .filter(|c| c.parsed_type.heading_level().is_some())
        .count();

    if heading_count < 3 {
        tracing::debug!(
            heading_count,
            "pdf_oxide: structure tree has too few heading elements (< 3), falling back to font-size clustering"
        );
        return Ok((Vec::new(), false));
    }

    let page_count = doc.doc.page_count().map_err(|e| {
        crate::pdf::error::PdfError::TextExtractionFailed(format!("pdf_oxide: failed to get page count: {e}"))
    })?;

    let mut all_pages: Vec<Vec<SegmentData>> = Vec::with_capacity(page_count);
    let mut total_role_assigned = 0usize;

    for page_idx in 0..page_count {
        let mcid_roles: HashMap<u32, Option<u8>> = all_page_content
            .get(&(page_idx as u32))
            .map(|contents| {
                contents
                    .iter()
                    .filter_map(|c| c.mcid.map(|mcid| (mcid, c.parsed_type.heading_level())))
                    .collect()
            })
            .unwrap_or_default();

        let segments = extract_segments_from_page_inner(doc, page_idx, &mcid_roles)?;
        total_role_assigned += segments.iter().filter(|s| s.assigned_role.is_some()).count();
        all_pages.push(segments);
    }

    tracing::debug!(
        page_count,
        total_role_assigned,
        "pdf_oxide: structure tree heading detection complete"
    );

    Ok((all_pages, true))
}

/// Extract text segments from all pages of a PDF document using pdf_oxide.
///
/// Attempts structure tree extraction first for tagged PDFs. Falls back to
/// plain font-metric extraction when the structure tree is unavailable or
/// unreliable.
///
/// Returns `(segments, used_structure_tree)` where the flag indicates whether
/// heading roles were pre-assigned from the structure tree.
///
/// # Arguments
///
/// * `doc` - Mutable reference to the oxide document
///
/// # Returns
///
/// Tuple of (per-page segment vectors, structure-tree-used flag).
pub(crate) fn extract_all_segments(doc: &mut OxideDocument) -> Result<(Vec<Vec<SegmentData>>, bool)> {
    let (tree_segments, used_tree) = extract_segments_with_structure_tree(doc)?;
    if used_tree && !tree_segments.is_empty() {
        return Ok((tree_segments, true));
    }

    let page_count = doc.doc.page_count().map_err(|e| {
        crate::pdf::error::PdfError::TextExtractionFailed(format!("pdf_oxide: failed to get page count: {e}"))
    })?;

    let mut all_pages: Vec<Vec<SegmentData>> = Vec::with_capacity(page_count);

    for page_idx in 0..page_count {
        let segments = extract_segments_from_page(doc, page_idx)?;
        all_pages.push(segments);
    }

    Ok((all_pages, false))
}

/// Extract `/Alt` (alternate description) text for `Figure` structure elements,
/// grouped by 0-based page index, in structure-tree document order (issue #62).
///
/// Tagged PDFs attach accessibility alt text to `Figure` structure elements via
/// the `/Alt` entry (ISO 32000-1:2008 §14.9.3), not to the image XObject itself.
/// Since the structure tree has no direct MCID/OBJR link back to a specific
/// `PdfImage` handle that callers can match on, this returns per-page alt-text
/// slots in tree order; callers pair the Nth `Figure` on a page with the Nth
/// image extracted from that page (both walk the page in document/paint order),
/// which holds for the common one-image-per-figure case.
///
/// Returns an empty map when the document has no `/StructTreeRoot`, no `/Pages`,
/// or either could not be read; this is not an error, most PDFs are untagged.
///
/// # Implementation note
///
/// This walks the raw `/StructTreeRoot` dictionary directly via `pdf_oxide`'s
/// low-level `Object`/`ObjectRef` accessors rather than `PdfDocument::structure_tree()`.
/// The latter is built for reading-order/heading detection and deliberately skips
/// parsing `/A` and `/Alt` (pdf_oxide's `structure::parser` module never populates
/// `StructElem::alt_text`), so it cannot be used here.
pub(crate) fn extract_figure_alt_text_by_page(doc: &mut OxideDocument) -> HashMap<u32, Vec<Option<String>>> {
    let mut by_page: HashMap<u32, Vec<Option<String>>> = HashMap::new();

    let Ok(catalog) = doc.doc.catalog() else {
        return by_page;
    };
    let Some(catalog_dict) = catalog.as_dict() else {
        return by_page;
    };

    let page_id_map = build_page_id_map(doc, catalog_dict);
    if page_id_map.is_empty() {
        return by_page;
    }

    let Some(struct_root_obj) = catalog_dict.get("StructTreeRoot") else {
        return by_page;
    };
    let struct_root = resolve_pdf_object(doc, struct_root_obj);
    let Some(struct_root_dict) = struct_root.as_dict() else {
        return by_page;
    };

    if let Some(k_obj) = struct_root_dict.get("K") {
        walk_struct_kids(doc, k_obj, &page_id_map, None, &mut by_page, 0);
    }

    by_page
}

/// Depth cap for `/Pages` and `/StructTreeRoot` tree walks, guarding against
/// cyclic or pathologically deep object graphs in untrusted PDF input.
const MAX_PDF_OBJECT_TREE_DEPTH: usize = 128;

/// Resolve `obj` to its underlying value if it is an indirect reference,
/// otherwise clone it. Unresolvable references degrade to `Object::Null`.
fn resolve_pdf_object(doc: &OxideDocument, obj: &pdf_oxide::object::Object) -> pdf_oxide::object::Object {
    match obj.as_reference() {
        Some(object_ref) => doc
            .doc
            .load_object(object_ref)
            .unwrap_or(pdf_oxide::object::Object::Null),
        None => obj.clone(),
    }
}

/// Build a map from PDF object id to 0-based page index by walking the
/// `/Pages` tree from the document catalog, in the same left-to-right,
/// depth-first order `page_count()`/`get_page(idx)` use. Generation numbers
/// are ignored (a `/Pg` reference and the corresponding `/Pages` leaf always
/// share the same object id per ISO 32000-1:2008 §7.3.10).
fn build_page_id_map(
    doc: &OxideDocument,
    catalog_dict: &HashMap<String, pdf_oxide::object::Object>,
) -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    let Some(pages_obj) = catalog_dict.get("Pages") else {
        return map;
    };
    let Some(pages_ref) = pages_obj.as_reference() else {
        return map;
    };

    let mut index = 0u32;
    let mut visited = std::collections::HashSet::new();
    walk_pages_tree(doc, pages_ref, &mut map, &mut index, &mut visited, 0);
    map
}

fn walk_pages_tree(
    doc: &OxideDocument,
    node_ref: pdf_oxide::object::ObjectRef,
    map: &mut HashMap<u32, u32>,
    index: &mut u32,
    visited: &mut std::collections::HashSet<u32>,
    depth: usize,
) {
    if depth > MAX_PDF_OBJECT_TREE_DEPTH || !visited.insert(node_ref.id) {
        return;
    }
    let Ok(node_obj) = doc.doc.load_object(node_ref) else {
        return;
    };
    let Some(node_dict) = node_obj.as_dict() else {
        return;
    };

    match node_dict.get("Kids").map(|kids_obj| resolve_pdf_object(doc, kids_obj)) {
        Some(pdf_oxide::object::Object::Array(kids)) => {
            for kid in &kids {
                if let Some(kid_ref) = kid.as_reference() {
                    walk_pages_tree(doc, kid_ref, map, index, visited, depth + 1);
                }
            }
        }
        _ => {
            // No /Kids (or an unresolvable one): this is a leaf page node.
            map.insert(node_ref.id, *index);
            *index += 1;
        }
    }
}

/// Walk a `/K` value of a structure element (or `StructTreeRoot`): a single
/// structure-element dict/reference, or an array mixing structure elements,
/// MCID integers, and marked-content-reference dicts. Only structure-element
/// entries (dicts carrying `/S`) are descended into.
fn walk_struct_kids(
    doc: &OxideDocument,
    k_obj: &pdf_oxide::object::Object,
    page_id_map: &HashMap<u32, u32>,
    inherited_page: Option<u32>,
    by_page: &mut HashMap<u32, Vec<Option<String>>>,
    depth: usize,
) {
    if depth > MAX_PDF_OBJECT_TREE_DEPTH {
        return;
    }
    match resolve_pdf_object(doc, k_obj) {
        pdf_oxide::object::Object::Array(items) => {
            for item in &items {
                walk_struct_elem(doc, item, page_id_map, inherited_page, by_page, depth + 1);
            }
        }
        resolved @ pdf_oxide::object::Object::Dictionary(_) => {
            walk_struct_elem_resolved(doc, &resolved, page_id_map, inherited_page, by_page, depth + 1);
        }
        _ => {}
    }
}

fn walk_struct_elem(
    doc: &OxideDocument,
    elem_obj: &pdf_oxide::object::Object,
    page_id_map: &HashMap<u32, u32>,
    inherited_page: Option<u32>,
    by_page: &mut HashMap<u32, Vec<Option<String>>>,
    depth: usize,
) {
    let resolved = resolve_pdf_object(doc, elem_obj);
    walk_struct_elem_resolved(doc, &resolved, page_id_map, inherited_page, by_page, depth);
}

/// Core of the structure-element walk: records `/Alt` for `Figure` elements,
/// then recurses into `/K` with the page inherited down for descendants that
/// omit their own `/Pg` (ISO 32000-1:2008 §14.7.2, Table 323).
fn walk_struct_elem_resolved(
    doc: &OxideDocument,
    resolved: &pdf_oxide::object::Object,
    page_id_map: &HashMap<u32, u32>,
    inherited_page: Option<u32>,
    by_page: &mut HashMap<u32, Vec<Option<String>>>,
    depth: usize,
) {
    if depth > MAX_PDF_OBJECT_TREE_DEPTH {
        return;
    }
    let Some(dict) = resolved.as_dict() else {
        return;
    };
    // Marked-content references and OBJR dicts have no /S; only real
    // structure elements do (ISO 32000-1:2008 §14.7.2).
    let Some(struct_type) = dict.get("S").and_then(|s| s.as_name()) else {
        return;
    };

    let own_page = dict
        .get("Pg")
        .and_then(|pg| pg.as_reference())
        .and_then(|pg_ref| page_id_map.get(&pg_ref.id).copied());
    let effective_page = own_page.or(inherited_page);

    if struct_type == "Figure"
        && let Some(page) = effective_page
    {
        let alt_text = dict
            .get("Alt")
            .and_then(|alt| alt.as_string())
            .and_then(super::metadata::decode_pdf_string);
        by_page.entry(page).or_default().push(alt_text);
    }

    if let Some(k_obj) = dict.get("K") {
        walk_struct_kids(doc, k_obj, page_id_map, effective_page, by_page, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use pdf_oxide::document::ReadingOrder;
    use pdf_oxide::geometry::Rect;
    use pdf_oxide::layout::TextSpan;

    use super::SegmentData;

    fn text_span(text: &str, x: f32, y: f32, width: f32) -> TextSpan {
        TextSpan {
            text: text.to_string(),
            bbox: Rect::new(x, y, width, 11.0),
            ..TextSpan::default()
        }
    }

    fn prose_columns() -> Vec<TextSpan> {
        let mut spans = Vec::new();
        for (index, text) in [
            "Left column has substantive prose",
            "Readers continue through this passage",
            "The final sentence completes support",
            "A fourth line strengthens the evidence",
            "The fifth line confirms a real column",
        ]
        .into_iter()
        .enumerate()
        {
            spans.push(text_span(text, 50.0, 700.0 - index as f32 * 18.0, 220.0));
        }
        for (index, text) in [
            "Right column also contains prose",
            "Its paragraph has balanced evidence",
            "Another sentence closes the column",
            "A fourth line continues on this side",
            "The fifth line completes the passage",
        ]
        .into_iter()
        .enumerate()
        {
            spans.push(text_span(text, 340.0, 700.0 - index as f32 * 18.0, 220.0));
        }
        spans
    }

    #[test]
    fn single_column_code_gutter_uses_top_to_bottom() {
        let mut spans = Vec::new();
        for index in 0..6 {
            spans.push(text_span(
                &(index + 1).to_string(),
                50.0,
                700.0 - index as f32 * 14.0,
                12.0,
            ));
            let mut code = text_span(
                "fn parse_value(input: &str) {",
                90.0,
                700.0 - index as f32 * 14.0,
                240.0,
            );
            code.is_monospace = true;
            spans.push(code);
        }
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 792.0),
            ReadingOrder::TopToBottom
        );
    }

    #[test]
    fn hanging_indent_uses_top_to_bottom() {
        let mut spans = vec![text_span("Note", 50.0, 700.0, 70.0)];
        for (index, text) in [
            "Indented prose continues across this line",
            "More text belongs to the same paragraph",
            "The hanging block remains one column",
            "Its last sentence provides enough length",
        ]
        .into_iter()
        .enumerate()
        {
            spans.push(text_span(text, 320.0, 700.0 - index as f32 * 18.0, 230.0));
        }
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 792.0),
            ReadingOrder::TopToBottom
        );
    }

    #[test]
    fn balanced_two_column_prose_uses_column_aware() {
        assert_eq!(
            super::select_reading_order(&prose_columns(), 612.0, 792.0),
            ReadingOrder::ColumnAware
        );
    }

    /// Page width shared by the GH#1397 element-path fixtures below, matching
    /// the other `select_reading_order` tests in this module.
    const GH1397_PAGE_WIDTH: f32 = 612.0;
    const GH1397_PAGE_HEIGHT: f32 = 792.0;

    /// Eight-row two-column prose page (GH#1397) with a gutter that clears
    /// `reorder_dense_two_column_page`'s own floor
    /// (`max(page_width * 0.02, 10.0)` = 12.24pt at `GH1397_PAGE_WIDTH`) but
    /// sits under pdf_oxide XY-Cut's default 15pt `min_valley_width`
    /// (`pdf_oxide::pipeline::XYCutStrategy::default().min_valley_width`) —
    /// the shape that let the reported document's `ColumnAware` and
    /// `TopToBottom` reading orders come out byte-identical: XY-Cut's own
    /// valley search never found a gutter this narrow.
    fn dense_two_column_spans_narrow_gutter() -> Vec<TextSpan> {
        const LEFT_X: f32 = 60.0;
        const LEFT_WIDTH: f32 = 200.0;
        const NARROW_GUTTER_PTS: f32 = 13.0;
        const RIGHT_X: f32 = LEFT_X + LEFT_WIDTH + NARROW_GUTTER_PTS;
        const RIGHT_WIDTH: f32 = 190.0;
        const ROW_HEIGHT_PTS: f32 = 14.0;
        const TOP_Y: f32 = 816.0;

        let left_body = [
            "The committee reviewed annual budget totals",
            "and approved new funding for the coming year",
            "after several rounds of careful review by",
            "senior staff members from every department",
            "who evaluated priorities across the whole",
            "organization before reaching a final decision",
            "that reflected both short and long term goals",
            "for sustainable growth across all programs",
        ];
        let right_body = [
            "Numerous studies have examined similar",
            "programs across comparable institutions",
            "using consistent methodology and controls",
            "for measuring outcomes over multiple years",
            "researchers found consistent positive trends",
            "supporting continued investment going forward",
            "additional citations appear in the appendix",
            "for readers seeking further detail here",
        ];

        let mut spans = Vec::new();
        for (row, (left_line, right_line)) in left_body.iter().copied().zip(right_body.iter().copied()).enumerate() {
            let y = TOP_Y - row as f32 * ROW_HEIGHT_PTS;
            spans.push(text_span(left_line, LEFT_X, y, LEFT_WIDTH));
            spans.push(text_span(right_line, RIGHT_X, y, RIGHT_WIDTH));
        }
        spans
    }

    /// GH#1397: the element-based extraction path (`extract_segments_from_page_inner`,
    /// via `reorder_page_reading_order`) must apply the same dense
    /// two-column band-split repair the plain-text path already used
    /// (`super::text::reorder_dense_two_column_page`), not rely solely on
    /// pdf_oxide's own `ColumnAware` XY-Cut pass.
    ///
    /// Revert check: reverting `reorder_page_reading_order` to its pre-fix
    /// form (drop the `reorder_dense_two_column_page` call and unconditionally
    /// run `apply_xy_cut_if_column_aware`, i.e. restore the old
    /// `extract_segments_from_page_inner` body) makes this test fail, because
    /// `select_reading_order` selects `ColumnAware` for this fixture (its
    /// gutter test only requires an 8pt gap) and XY-Cut's own 15pt
    /// `min_valley_width` floor then leaves the 13pt-gutter page in
    /// unrepaired top-to-bottom (row-interleaved) order.
    #[test]
    fn element_path_reorders_narrow_gutter_dense_two_column_page() {
        let mut spans = dense_two_column_spans_narrow_gutter();

        super::reorder_page_reading_order(&mut spans, GH1397_PAGE_WIDTH, GH1397_PAGE_HEIGHT, 0);

        let texts: Vec<&str> = spans.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "The committee reviewed annual budget totals",
                "and approved new funding for the coming year",
                "after several rounds of careful review by",
                "senior staff members from every department",
                "who evaluated priorities across the whole",
                "organization before reaching a final decision",
                "that reflected both short and long term goals",
                "for sustainable growth across all programs",
                "Numerous studies have examined similar",
                "programs across comparable institutions",
                "using consistent methodology and controls",
                "for measuring outcomes over multiple years",
                "researchers found consistent positive trends",
                "supporting continued investment going forward",
                "additional citations appear in the appendix",
                "for readers seeking further detail here",
            ],
            "dense two-column band-split repair must run in the element path and group each column together"
        );
    }

    /// Regression guard for GH#1397: a genuine single-column page (numbered
    /// lines beside monospace code, from `single_column_code_gutter_uses_top_to_bottom`
    /// above) must come out of `reorder_page_reading_order` byte-identical to
    /// its input order. `reorder_dense_two_column_page`'s own gate rejects it
    /// (pdf_oxide's region classifier does not treat monospace code as a
    /// reorderable prose column), and `select_reading_order` already returns
    /// `TopToBottom` for this shape, so XY-Cut is never invoked either.
    #[test]
    fn element_path_leaves_single_column_code_page_unchanged() {
        let mut spans = Vec::new();
        for index in 0..6 {
            spans.push(text_span(
                &(index + 1).to_string(),
                50.0,
                700.0 - index as f32 * 14.0,
                12.0,
            ));
            let mut code = text_span(
                "fn parse_value(input: &str) {",
                90.0,
                700.0 - index as f32 * 14.0,
                240.0,
            );
            code.is_monospace = true;
            spans.push(code);
        }
        let original: Vec<String> = spans.iter().map(|span| span.text.clone()).collect();

        super::reorder_page_reading_order(&mut spans, GH1397_PAGE_WIDTH, GH1397_PAGE_HEIGHT, 0);

        let texts: Vec<&str> = spans.iter().map(|span| span.text.as_str()).collect();
        assert_eq!(
            texts,
            original.iter().map(String::as_str).collect::<Vec<_>>(),
            "a single-column page must not be reordered by either the dense repair or XY-Cut"
        );
    }

    #[test]
    fn full_width_heading_does_not_hide_prose_columns() {
        let mut spans = prose_columns();
        let mut heading = text_span("A Full Width Heading", 50.0, 750.0, 510.0);
        heading.heading_level = Some(1);
        spans.push(heading);
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 792.0),
            ReadingOrder::ColumnAware
        );
    }

    #[test]
    fn table_grid_is_not_prose_column_evidence() {
        let mut spans = Vec::new();
        for row in 0..4 {
            let y = 700.0 - row as f32 * 18.0;
            spans.push(text_span("Regional revenue", 50.0, y, 90.0));
            spans.push(text_span("annual total", 160.0, y, 80.0));
            spans.push(text_span("Operating expense", 340.0, y, 90.0));
            spans.push(text_span("annual total", 450.0, y, 80.0));
        }
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 792.0),
            ReadingOrder::TopToBottom
        );
    }

    #[test]
    fn nonfinite_and_degenerate_geometry_is_deterministically_top_to_bottom() {
        let mut spans = prose_columns();
        spans[0].bbox.x = f32::NAN;
        assert_eq!(
            super::select_reading_order(&spans, f32::NAN, 792.0),
            ReadingOrder::TopToBottom
        );
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 0.0),
            ReadingOrder::TopToBottom
        );
    }

    #[test]
    fn academic_columns_with_crossing_regions_use_column_aware() {
        const FIXTURE_LEFT_SUPPORTED_LINES: usize = 7;
        const FIXTURE_RIGHT_SUPPORTED_LINES: usize = 26;
        const FIXTURE_CROSSING_SPANS: usize = 37;

        let mut spans = Vec::new();
        for index in 0..FIXTURE_LEFT_SUPPORTED_LINES {
            spans.push(text_span(
                "Left academic prose remains substantive",
                50.0,
                680.0 - index as f32 * 16.0,
                220.0,
            ));
        }
        for index in 0..FIXTURE_RIGHT_SUPPORTED_LINES {
            spans.push(text_span(
                "Right academic prose continues beside figures",
                340.0,
                700.0 - index as f32 * 16.0,
                220.0,
            ));
        }
        for index in 0..FIXTURE_CROSSING_SPANS {
            spans.push(text_span(
                "Cross-column title author or figure content",
                50.0,
                760.0 - index as f32 * 8.0,
                510.0,
            ));
        }
        assert_eq!(
            super::select_reading_order(&spans, 612.0, 792.0),
            ReadingOrder::ColumnAware
        );
    }

    fn seg(text: &str, x: f32, y: f32, font_size: f32, is_bold: bool) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x,
            y,
            width: text.len() as f32 * font_size * 0.5,
            height: font_size,
            font_size,
            is_bold,
            is_italic: false,
            is_monospace: false,
            baseline_y: y,
            assigned_role: None,
        }
    }

    fn positioned_span(text: &str, x: f32, y: f32, width: f32, font_size: f32, char_x_offsets: Vec<f32>) -> TextSpan {
        TextSpan {
            text: text.to_string(),
            bbox: Rect::new(x, y, width, font_size),
            font_size,
            char_x_offsets,
            ..TextSpan::default()
        }
    }

    #[test]
    fn should_interleave_internal_and_suffix_subscripts_from_character_geometry() {
        let base = positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![100.0, 105.0, 110.0, 115.0]);
        let trailing = positioned_span(" solution", 125.0, 200.0, 40.0, 10.0, vec![]);
        let subscript_two = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);
        let subscript_four = positioned_span("4", 120.0, 198.4, 3.0, 6.7, vec![120.0]);

        let repaired = super::rejoin_inline_scripts(vec![base, trailing, subscript_two, subscript_four]);
        let texts: Vec<_> = repaired.iter().map(|span| span.text.as_str()).collect();

        assert_eq!(texts, ["H", "2", " SO4", " solution"]);
        assert_eq!(repaired[1].bbox.y, 200.0);
        assert_eq!(repaired[1].font_size, 10.0);
        assert_eq!(repaired[2].bbox.y, 200.0);
        assert_eq!(repaired[2].font_size, 10.0);
    }

    #[test]
    fn should_use_complete_character_widths_when_origins_are_unavailable() {
        let mut base = positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![]);
        base.char_widths = vec![4.5, 4.5, 4.5, 4.5];
        let trailing = positioned_span(" solution", 125.0, 200.0, 40.0, 10.0, vec![]);
        let subscript = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);

        let repaired = super::rejoin_inline_scripts(vec![base, trailing, subscript]);
        let texts: Vec<_> = repaired.iter().map(|span| span.text.as_str()).collect();

        assert_eq!(texts, ["H", "2", " SO", " solution"]);
    }

    #[test]
    fn should_keep_adjacent_same_baseline_unit_suffix_inline() {
        let base = positioned_span("A/cm", 100.0, 200.0, 20.0, 10.0, vec![]);
        let exponent = positioned_span("2", 120.1, 200.0, 3.0, 6.7, vec![120.1]);
        let closing = positioned_span(")", 123.2, 200.0, 3.0, 10.0, vec![123.2]);

        let repaired = super::rejoin_inline_scripts(vec![base, exponent, closing]);
        let texts: Vec<_> = repaired.iter().map(|span| span.text.as_str()).collect();

        assert_eq!(texts, ["A/cm2", ")"]);
        assert_eq!(repaired[0].font_size, 10.0);
    }

    #[test]
    fn should_preserve_native_order_when_internal_character_offsets_are_missing() {
        let base = positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![]);
        let trailing = positioned_span(" solution", 125.0, 200.0, 40.0, 10.0, vec![]);
        let subscript = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);

        let repaired = super::rejoin_inline_scripts(vec![base, trailing, subscript]);
        let texts: Vec<_> = repaired.iter().map(|span| span.text.as_str()).collect();

        assert_eq!(texts, ["H SO", " solution", "2"]);
        assert_eq!(repaired[2].font_size, 6.7);
    }

    #[test]
    fn should_preserve_native_order_for_non_strict_character_geometry() {
        for mut base in [
            positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![100.0, 105.0, 105.0, 115.0]),
            positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![]),
        ] {
            if base.char_x_offsets.is_empty() {
                base.char_widths = vec![5.0, 0.0, 5.0, 5.0];
            }
            let trailing = positioned_span(" solution", 125.0, 200.0, 40.0, 10.0, vec![]);
            let subscript = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);

            let repaired = super::rejoin_inline_scripts(vec![base, trailing, subscript]);
            assert_eq!(
                repaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
                ["H SO", " solution", "2"]
            );
        }
    }

    #[test]
    fn should_not_join_separate_same_baseline_table_cells() {
        let label = positioned_span("Total", 100.0, 200.0, 25.0, 10.0, vec![]);
        let cell = positioned_span("2", 150.0, 200.0, 3.0, 6.7, vec![150.0]);

        let repaired = super::rejoin_inline_scripts(vec![label, cell]);

        assert_eq!(repaired.len(), 2);
        assert_eq!(repaired[1].font_size, 6.7);
    }

    #[test]
    fn should_return_original_allocation_when_no_scripts_attach() {
        let spans = vec![
            positioned_span("alpha", 10.0, 20.0, 25.0, 10.0, vec![0.0, 5.0, 10.0, 15.0, 20.0]),
            positioned_span("beta", 40.0, 20.0, 20.0, 10.0, vec![0.0, 5.0, 10.0, 15.0]),
        ];
        let original_allocation = spans.as_ptr();

        let repaired = super::rejoin_inline_scripts(spans);

        assert_eq!(repaired.as_ptr(), original_allocation);
        assert_eq!(repaired.len(), 2);
        assert_eq!(repaired[0].text, "alpha");
        assert_eq!(repaired[1].text, "beta");
    }

    #[test]
    fn should_not_reorder_rotated_vertical_or_rtl_spans() {
        for configure in [
            |span: &mut TextSpan| span.rotation_degrees = 90.0,
            |span: &mut TextSpan| span.wmode = 1,
            |span: &mut TextSpan| span.rtl_draw_logical = true,
        ] {
            let mut base = positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![100.0, 105.0, 110.0, 115.0]);
            configure(&mut base);
            let trailing = positioned_span(" solution", 125.0, 200.0, 40.0, 10.0, vec![]);
            let subscript = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);

            let repaired = super::rejoin_inline_scripts(vec![base, trailing, subscript]);
            let texts: Vec<_> = repaired.iter().map(|span| span.text.as_str()).collect();

            assert_eq!(texts, ["H SO", " solution", "2"]);
        }
    }

    #[test]
    fn should_bound_and_normalize_text_rise_against_base_font() {
        let base = positioned_span("Unit", 100.0, 200.0, 20.0, 10.0, vec![]);
        let separator = positioned_span(" tail", 130.0, 200.0, 20.0, 10.0, vec![]);
        let mut insufficient_rise = positioned_span("2", 120.1, 200.0, 3.0, 6.7, vec![120.1]);
        insufficient_rise.text_rise = 0.1;
        let unrepaired = super::rejoin_inline_scripts(vec![base.clone(), separator.clone(), insufficient_rise]);
        assert_eq!(
            unrepaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Unit", " tail", "2"]
        );

        let mut normalized_rise = positioned_span("2", 120.1, 200.0, 3.0, 6.7, vec![120.1]);
        normalized_rise.text_rise = 0.15;
        let repaired = super::rejoin_inline_scripts(vec![base.clone(), separator.clone(), normalized_rise]);
        assert_eq!(
            repaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Unit2", " tail"]
        );

        let mut distant = positioned_span("2", 120.1, 195.0, 3.0, 6.7, vec![120.1]);
        distant.text_rise = 0.3;
        let bounded = super::rejoin_inline_scripts(vec![base, separator, distant]);
        assert_eq!(
            bounded.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Unit", " tail", "2"]
        );
    }

    #[test]
    fn should_never_attach_or_leak_artifact_spans() {
        let base = positioned_span("Unit", 100.0, 200.0, 20.0, 10.0, vec![]);
        let mut artifact_script = positioned_span("2", 120.1, 198.4, 3.0, 6.7, vec![120.1]);
        artifact_script.artifact_type = Some(pdf_oxide::extractors::text::ArtifactType::Layout);
        let repaired = super::rejoin_inline_scripts(vec![base, artifact_script]);
        assert_eq!(repaired[0].text, "Unit");
        assert_eq!(repaired[1].text, "2");
        assert!(repaired[1].artifact_type.is_some());

        let mut artifact_base = positioned_span("Unit", 100.0, 200.0, 20.0, 10.0, vec![]);
        artifact_base.artifact_type = Some(pdf_oxide::extractors::text::ArtifactType::Layout);
        let script = positioned_span("2", 120.1, 198.4, 3.0, 6.7, vec![120.1]);
        let repaired = super::rejoin_inline_scripts(vec![artifact_base, script]);
        assert_eq!(repaired.len(), 2);
        assert!(repaired[0].artifact_type.is_some());
    }

    #[test]
    fn should_choose_nearest_baseline_then_nearest_native_base() {
        let closer_old = positioned_span("Old", 100.0, 199.0, 20.0, 10.0, vec![]);
        let farther_recent = positioned_span("New", 100.0, 200.0, 20.0, 10.0, vec![]);
        let script = positioned_span("2", 120.1, 197.5, 3.0, 6.7, vec![120.1]);
        let repaired = super::rejoin_inline_scripts(vec![closer_old, farther_recent, script]);
        assert_eq!(
            repaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Old2", "New"]
        );

        let old_tie = positioned_span("Old", 100.0, 200.0, 20.0, 10.0, vec![]);
        let recent_tie = positioned_span("New", 100.0, 200.0, 20.0, 10.0, vec![]);
        let script = positioned_span("2", 120.1, 198.4, 3.0, 6.7, vec![120.1]);
        let repaired = super::rejoin_inline_scripts(vec![old_tie, recent_tie, script]);
        assert_eq!(
            repaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Old", "New2"]
        );
    }

    #[test]
    fn should_not_attach_to_a_span_that_is_already_a_script() {
        let base = positioned_span("A", 100.0, 200.0, 10.0, 10.0, vec![]);
        let script_base = positioned_span("b", 110.1, 198.4, 3.0, 6.7, vec![110.1]);
        let nested_script = positioned_span("c", 113.2, 197.2, 2.0, 4.2, vec![113.2]);
        let repaired = super::rejoin_inline_scripts(vec![base, script_base, nested_script]);
        assert_eq!(
            repaired.iter().map(|span| span.text.as_str()).collect::<Vec<_>>(),
            ["Ab", "c"]
        );
    }

    #[test]
    fn should_normalize_hierarchy_metadata_to_the_base() {
        let mut base = positioned_span("H SO", 100.0, 200.0, 20.0, 10.0, vec![100.0, 105.0, 110.0, 115.0]);
        base.font_name = "BaseFont".to_string();
        base.font_weight = pdf_oxide::layout::text_block::FontWeight::Bold;
        base.is_italic = true;
        base.is_monospace = true;
        base.mcid = Some(7);
        base.heading_level = Some(2);
        let script = positioned_span("2", 105.0, 198.4, 3.0, 6.7, vec![105.0]);

        let repaired = super::rejoin_inline_scripts(vec![base, script]);
        let normalized = &repaired[1];
        assert_eq!(normalized.font_name, "BaseFont");
        assert_eq!(normalized.font_weight, pdf_oxide::layout::text_block::FontWeight::Bold);
        assert!(normalized.is_italic);
        assert!(normalized.is_monospace);
        assert_eq!(normalized.mcid, Some(7));
        assert_eq!(normalized.heading_level, Some(2));
        assert_eq!(normalized.text_rise, 0.0);
    }

    #[test]
    fn should_collapse_exact_redrawn_duplicate() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("Duplicated", 72.0, 700.0, 14.0, false),
            seg("Duplicated", 72.0, 700.0, 14.0, false),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Duplicated");
    }

    #[test]
    fn should_collapse_shifted_duplicate_and_absorb_bold_and_size() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("Weight", 72.0, 650.0, 14.0, false),
            seg("Weight", 72.6, 649.5, 15.0, true),
        ]);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_bold, "double-draw bold signal must be kept");
        assert_eq!(out[0].font_size, 15.0, "larger draw wins the size signal");
    }

    #[test]
    fn should_collapse_issue_1114_shift_variants() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("Horizontal shift", 117.6, 237.0, 18.0, false),
            seg("Horizontal shift", 123.3, 237.0, 18.0, false),
            seg("Vertical shift", 117.6, 187.1, 18.0, false),
            seg("Vertical shift", 117.6, 183.4, 18.0, false),
        ]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn should_keep_identical_digits_in_adjacent_table_cells() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("1", 100.0, 500.0, 10.0, false),
            seg("1", 106.0, 500.0, 10.0, false),
            seg("1", 100.0, 488.0, 10.0, false),
        ]);
        assert_eq!(out.len(), 3, "adjacent identical table cells are real text");
    }

    #[test]
    fn should_keep_repeated_word_at_distinct_position() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("total", 72.0, 700.0, 10.0, false),
            seg("total", 140.0, 700.0, 10.0, false),
            seg("total", 72.0, 640.0, 10.0, false),
        ]);
        assert_eq!(out.len(), 3, "same word at different positions is real text");
    }

    #[test]
    fn should_keep_different_text_at_same_position() {
        let out = super::dedupe_redrawn_segments(vec![
            seg("a", 72.0, 700.0, 10.0, false),
            seg("b", 72.0, 700.0, 10.0, false),
        ]);
        assert_eq!(out.len(), 2);
    }
}
