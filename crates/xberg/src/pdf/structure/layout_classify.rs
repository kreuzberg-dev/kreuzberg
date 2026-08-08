//! Layout-detection-based paragraph classification overrides.
//!
//! When layout detection is enabled, this module applies layout hints
//! to override or augment the font-size-based paragraph classification
//! from the standard markdown pipeline.

use super::geometry::Rect;
use super::types::{LayoutHint, LayoutHintClass, PdfParagraph};

const COMPARABLE_CONTAINMENT_TOLERANCE: f32 = 0.05;
const MAX_PROSE_CODE_SYNTAX_RATIO: f64 = 0.03;
const CODE_HEADING_OVERRIDE_CONFIDENCE: f32 = 0.8;
const MIN_STRUCTURED_CODE_SYNTAX_CHARACTERS: usize = 3;
const MIN_CODE_ASSIGNMENT_OPERATORS: usize = 2;

/// Apply layout detection overrides to classified paragraphs.
///
/// Uses two matching strategies:
/// 1. **Spatial matching** (heuristic pages): computes bounding boxes from segment
///    positions and matches by containment overlap.
/// 2. **Proportional matching** (structure tree pages): paragraphs without positional
///    data are matched to hints by estimated vertical position, since both are in
///    reading order.
///
/// Structure-tree headings are preserved: only paragraphs without existing
/// heading classification receive heading overrides from layout detection.
pub(crate) fn apply_layout_overrides(
    paragraphs: &mut [PdfParagraph],
    hints: &[LayoutHint],
    min_confidence: f32,
    min_containment: f32,
    body_font_size: Option<f32>,
) {
    let _ = apply_layout_overrides_with_matches(paragraphs, hints, min_confidence, min_containment, body_font_size);
}

/// Record spatial layout classes without changing native paragraph semantics.
///
/// This supports consumers that need region provenance (for example guarded
/// table-spill cleanup) while preserving font- and tag-derived headings, lists,
/// code, and formulas.
#[cfg(feature = "layout-detection")]
pub(crate) fn annotate_layout_classes(
    paragraphs: &mut [PdfParagraph],
    hints: &[LayoutHint],
    min_confidence: f32,
    min_containment: f32,
) {
    for paragraph in paragraphs {
        if let Some((_, hint, _)) = best_spatial_match(paragraph, hints, min_confidence, min_containment) {
            paragraph.layout_class = Some(hint.class_name);
        }
    }
}

pub(crate) fn apply_layout_overrides_with_matches(
    paragraphs: &mut [PdfParagraph],
    hints: &[LayoutHint],
    min_confidence: f32,
    min_containment: f32,
    body_font_size: Option<f32>,
) -> Vec<Option<usize>> {
    if hints.is_empty() {
        return vec![None; paragraphs.len()];
    }

    let has_any_positions = paragraphs.iter().any(|p| compute_paragraph_bbox(p).is_some());
    let matches = if has_any_positions {
        apply_spatial_overrides_with_matches(paragraphs, hints, min_confidence, min_containment, body_font_size)
    } else {
        tracing::debug!("Skipping proportional layout overrides: structure tree pages use font-size classification");
        vec![None; paragraphs.len()]
    };

    trace_layout_summary(paragraphs);
    matches
}

fn trace_layout_summary(paragraphs: &[PdfParagraph]) {
    tracing::debug!(
        total = paragraphs.len(),
        headings = paragraphs.iter().filter(|p| p.heading_level.is_some()).count(),
        list_items = paragraphs.iter().filter(|p| p.is_list_item).count(),
        code_blocks = paragraphs.iter().filter(|p| p.is_code_block).count(),
        formulas = paragraphs.iter().filter(|p| p.is_formula).count(),
        furniture = paragraphs.iter().filter(|p| p.is_page_furniture).count(),
        "layout overrides applied"
    );
}

/// Spatial matching: match paragraphs to hints by bounding box overlap.
///
/// Uses a two-tier strategy:
/// 1. **2D containment** (intersection_area / paragraph_area): best for paragraphs
///    that horizontally overlap with the layout hint.
/// 2. **Vertical-only overlap** (vertical_intersection / paragraph_height): fallback
///    for paragraphs where horizontal alignment differs (e.g., centered text vs
///    left-aligned detection box).
///
/// The vertical fallback requires higher confidence to reduce false positives.
///
/// For promotion classes (Title, SectionHeader, Caption, Footnote, ListItem), also
/// validates text content matches the hint type: e.g., SectionHeader hints only apply
/// to short paragraphs (≤200 chars), ListItem hints to list marker prefixes. This
/// prevents false promotion of long body paragraphs that happen to spatially overlap
/// a heading hint.
fn apply_spatial_overrides_with_matches(
    paragraphs: &mut [PdfParagraph],
    hints: &[LayoutHint],
    min_confidence: f32,
    min_containment: f32,
    body_font_size: Option<f32>,
) -> Vec<Option<usize>> {
    let mut matches = vec![None; paragraphs.len()];
    for (para_idx, para) in paragraphs.iter_mut().enumerate() {
        if let Some((hint_index, hint, containment)) = best_spatial_match(para, hints, min_confidence, min_containment)
        {
            tracing::trace!(
                para_idx,
                hint_class = ?hint.class_name,
                containment,
                "spatial hint match"
            );
            apply_hint_to_paragraph(para, hint, body_font_size);
            matches[para_idx] = Some(hint_index);
        }
    }
    matches
}

fn best_spatial_match<'a>(
    paragraph: &PdfParagraph,
    hints: &'a [LayoutHint],
    min_confidence: f32,
    min_containment: f32,
) -> Option<(usize, &'a LayoutHint, f32)> {
    let paragraph_bbox = compute_paragraph_bbox(paragraph)?;
    if paragraph_bbox.height() <= 0.0 {
        return None;
    }
    let paragraph_text = paragraph_text(paragraph);
    let matches = hints.iter().enumerate().filter_map(|(index, hint)| {
        let hint_rect = Rect::from_lbrt(hint.left, hint.bottom, hint.right, hint.top);
        let containment = paragraph_bbox.intersection_over_self(&hint_rect);
        (hint.confidence >= min_confidence
            && containment >= min_containment
            && matches_hint_text(hint, &paragraph_text))
        .then_some((index, hint, containment))
    });
    let candidates = matches.collect::<Vec<_>>();
    let maximum = candidates
        .iter()
        .map(|(_, _, containment)| *containment)
        .max_by(f32::total_cmp)?;
    candidates
        .into_iter()
        .filter(|(_, _, containment)| maximum - containment <= COMPARABLE_CONTAINMENT_TOLERANCE)
        .max_by(compare_spatial_matches)
}

fn compare_spatial_matches(a: &(usize, &LayoutHint, f32), b: &(usize, &LayoutHint, f32)) -> std::cmp::Ordering {
    semantic_hint_priority(a.1.class_name)
        .cmp(&semantic_hint_priority(b.1.class_name))
        .then_with(|| a.1.confidence.total_cmp(&b.1.confidence))
        .then_with(|| hint_area(b.1).total_cmp(&hint_area(a.1)))
        .then_with(|| a.2.total_cmp(&b.2))
        .then_with(|| layout_hint_class_rank(a.1.class_name).cmp(&layout_hint_class_rank(b.1.class_name)))
        .then_with(|| a.1.left.total_cmp(&b.1.left))
        .then_with(|| a.1.bottom.total_cmp(&b.1.bottom))
        .then_with(|| a.1.right.total_cmp(&b.1.right))
        .then_with(|| a.1.top.total_cmp(&b.1.top))
}

fn semantic_hint_priority(class_name: LayoutHintClass) -> u8 {
    match class_name {
        LayoutHintClass::Title
        | LayoutHintClass::SectionHeader
        | LayoutHintClass::ListItem
        | LayoutHintClass::Caption
        | LayoutHintClass::Footnote => 2,
        _ => 1,
    }
}

fn layout_hint_class_rank(class_name: LayoutHintClass) -> u8 {
    match class_name {
        LayoutHintClass::Title => 0,
        LayoutHintClass::SectionHeader => 1,
        LayoutHintClass::Code => 2,
        LayoutHintClass::Formula => 3,
        LayoutHintClass::ListItem => 4,
        LayoutHintClass::Caption => 5,
        LayoutHintClass::Footnote => 6,
        LayoutHintClass::PageHeader => 7,
        LayoutHintClass::PageFooter => 8,
        LayoutHintClass::Table => 9,
        LayoutHintClass::Picture => 10,
        LayoutHintClass::DocumentIndex => 11,
        LayoutHintClass::Form => 12,
        LayoutHintClass::KeyValueRegion => 13,
        LayoutHintClass::Text => 14,
        LayoutHintClass::Other => 15,
    }
}

fn hint_area(hint: &LayoutHint) -> f32 {
    (hint.right - hint.left).max(0.0) * (hint.top - hint.bottom).max(0.0)
}

/// Check if text matches the content expectations of a layout hint class.
///
/// For promotion classes (Title, SectionHeader, Caption, Footnote, ListItem),
/// validate that the paragraph content aligns with the hint type:
/// - Title/SectionHeader/Caption/Footnote: short text (≤200 chars)
/// - ListItem: text starts with list marker (digit, bullet, dash, etc.)
/// - Other classes: always match (no text constraint)
fn matches_hint_text(hint: &LayoutHint, para_text: &str) -> bool {
    use LayoutHintClass as L;
    match hint.class_name {
        L::Title | L::SectionHeader => para_text.chars().count() <= 200,
        L::Caption | L::Footnote => para_text.chars().count() <= 200,
        L::ListItem => {
            let trimmed = para_text.trim_start();
            trimmed.starts_with(|c: char| c.is_ascii_digit())
                || trimmed.starts_with('•')
                || trimmed.starts_with('-')
                || trimmed.starts_with('*')
                || trimmed.starts_with('·')
        }
        L::Formula => has_formula_evidence(para_text),
        _ => true,
    }
}

fn has_formula_evidence(text: &str) -> bool {
    let total_chars = text.chars().count();
    if total_chars == 0 {
        return false;
    }
    let math_chars = text
        .chars()
        .filter(|character| {
            matches!(
                character,
                '+' | '='
                    | '^'
                    | '∑'
                    | '∫'
                    | '∏'
                    | '√'
                    | '∞'
                    | '≤'
                    | '≥'
                    | '≠'
                    | '≈'
                    | '±'
                    | '×'
                    | '÷'
                    | '∂'
                    | '∇'
            )
        })
        .count();
    math_chars >= 3 || (math_chars as f64 / total_chars as f64) >= 0.15
}

/// Extract full text from a paragraph.
fn paragraph_text(para: &PdfParagraph) -> String {
    if !para.text.is_empty() {
        para.text.clone()
    } else {
        para.lines
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Check if text is a separator/filler line (dashes, underscores, tildes, etc.)
/// that should never be classified as a heading.
pub(super) fn is_separator_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let total = trimmed.chars().count();
    let alnum = trimmed.chars().filter(|c| c.is_alphanumeric()).count();
    if alnum == 0 {
        return true;
    }
    total >= 6 && (alnum as f64 / total as f64) < 0.15
}

/// Infer heading level from section numbering in the text.
///
/// Academic papers use numbering to indicate heading depth:
/// - "1 Introduction" → H2 (top-level section)
/// - "3.2 AI models" → H3 (sub-section)
/// - "3.2.1 Details" → H4 (sub-sub-section)
/// - "Layout Analysis Model" (no number) → H2 (default for SectionHeader)
pub(super) fn infer_heading_level_from_text(text: &str, hint_class: LayoutHintClass) -> u8 {
    if hint_class == LayoutHintClass::Title {
        return 1;
    }

    let trimmed = text.trim();

    let first_char = trimmed.chars().next().unwrap_or(' ');
    let is_alpha_prefix = first_char.is_ascii_alphabetic()
        && trimmed.len() >= 2
        && matches!(trimmed.as_bytes().get(1), Some(b'.' | b')' | b' '));

    let numbering_end = if is_alpha_prefix {
        let after_letter = &trimmed[1..];
        let rest_end = after_letter
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(0);
        1 + rest_end
    } else {
        trimmed.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(0)
    };

    if numbering_end == 0 {
        return 2;
    }

    let numbering = &trimmed[..numbering_end];
    let dot_count = numbering.chars().filter(|&c| c == '.').count();

    let effective_dots = if numbering.ends_with('.') {
        dot_count.saturating_sub(1)
    } else {
        dot_count
    };

    match effective_dots {
        0 => 2,
        1 => 3,
        _ => 4,
    }
}

/// Apply a single hint's classification to a paragraph.
///
/// `body_font_size`: when provided, used to guard against promoting body-text-sized
/// paragraphs to headings (unnumbered SectionHeader at body font size is likely a
/// false positive from the layout model).
pub(super) fn apply_hint_to_paragraph(para: &mut PdfParagraph, hint: &LayoutHint, body_font_size: Option<f32>) {
    tracing::debug!(
        hint_class = ?hint.class_name,
        confidence = hint.confidence,
        old_heading = ?para.heading_level,
        "applying layout hint"
    );

    para.layout_class = Some(hint.class_name);

    let debug = super::layout_debug::layout_debug_flags();
    let old_heading = para.heading_level;

    let para_text: String = if !para.text.is_empty() {
        para.text.clone()
    } else {
        para.lines
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let word_count = para_text.split_whitespace().count();
    let is_sep = is_separator_text(&para_text);

    // Independent heading evidence: font clearly above body, bold weight, or a recognized
    // section-numbering pattern. Used to veto destructive demotion (A2) and for override
    // logging. Computed once so the guard and the log block agree. ~keep
    let font_above_body = body_font_size.is_some_and(|body| body > 0.0 && para.dominant_font_size > body + 0.5);
    let has_strong_heading_evidence =
        font_above_body || para.is_bold || super::classify::is_section_pattern(para_text.trim());

    match hint.class_name {
        LayoutHintClass::Title
            if !debug.no_promote
                && !is_sep
                && !para.is_list_item
                && (para.heading_level.is_none() || hint.confidence >= 0.7)
                && word_count <= super::constants::MAX_HEADING_WORD_COUNT =>
        {
            para.heading_level = Some(1);
        }
        LayoutHintClass::SectionHeader
            if !debug.no_promote
                && !is_sep
                && !para.is_list_item
                && (para.heading_level.is_none() || hint.confidence >= 0.7) =>
        {
            let trimmed = para_text.trim();
            let too_long = word_count > super::constants::MAX_HEADING_WORD_COUNT;
            let ends_period = trimmed.ends_with('.') && !super::classify::is_section_pattern(trimmed);
            let ends_colon = trimmed.ends_with(':');
            let is_figure = super::regions::looks_like_figure_label(trimmed);
            let is_monospace = if !para.text.is_empty() {
                para.is_monospace_hint()
            } else {
                para.lines.iter().all(|l| l.is_monospace)
            };
            let text_level = infer_heading_level_from_text(&para_text, hint.class_name);
            let near_body = body_font_size.is_some_and(|body| {
                body > 0.0 && para.dominant_font_size >= body - 1.5 && para.dominant_font_size <= body + 0.5
            });
            let is_unnumbered = text_level == 2;
            let high_confidence_bold = hint.confidence >= 0.7 && para.is_bold;
            let looks_like_sentence = trimmed.ends_with('.') && word_count > 8;
            let body_size_guard = near_body && is_unnumbered && (!high_confidence_bold || looks_like_sentence);
            if !too_long && !ends_period && !ends_colon && !is_figure && !is_monospace && !body_size_guard {
                para.heading_level = Some(text_level);
            }
        }
        LayoutHintClass::Code => {
            let sentence_endings = para_text
                .chars()
                .filter(|&c| c == '.' || c == '!' || c == '?' || c == ',')
                .count();
            let syntax_chars = para_text
                .chars()
                .filter(|c| {
                    matches!(
                        c,
                        '{' | '}' | '(' | ')' | '[' | ']' | ';' | '=' | '<' | '>' | '|' | '@' | '#' | '$'
                    )
                })
                .count();
            let syntax_ratio = if para_text.is_empty() {
                0.0
            } else {
                syntax_chars as f64 / para_text.len() as f64
            };
            let is_prose = sentence_endings >= 2 && syntax_ratio < MAX_PROSE_CODE_SYNTAX_RATIO && word_count > 15;
            let assignment_operators = para_text.chars().filter(|&character| character == '=').count();
            let has_structured_code_syntax = syntax_chars >= MIN_STRUCTURED_CODE_SYNTAX_CHARACTERS
                && (para_text.chars().any(|character| matches!(character, '{' | '}' | ';'))
                    || assignment_operators >= MIN_CODE_ASSIGNMENT_OPERATORS);
            let preserves_heading = para.heading_level.is_some()
                && has_strong_heading_evidence
                && hint.confidence < CODE_HEADING_OVERRIDE_CONFIDENCE
                && !has_structured_code_syntax;
            if !is_prose && !preserves_heading && !para.is_list_item {
                para.is_code_block = true;
                para.heading_level = None;
            }
        }
        LayoutHintClass::Formula => {
            para.is_formula = true;
            para.heading_level = None;
        }
        LayoutHintClass::ListItem if hint.confidence >= 0.8 => {
            para.is_list_item = true;
        }
        LayoutHintClass::PageHeader | LayoutHintClass::PageFooter if para.heading_level.is_none() => {
            para.is_page_furniture = hint.confidence >= 0.8;
        }
        LayoutHintClass::Picture if para.heading_level.is_none() => {
            para.is_page_furniture = true;
        }
        LayoutHintClass::Text | LayoutHintClass::Caption | LayoutHintClass::Footnote
            if !debug.no_demote
                && para.heading_level.is_some()
                && !has_strong_heading_evidence
                && hint.confidence >= super::constants::HEADING_DEMOTE_CONFIDENCE =>
        {
            tracing::trace!(
                hint_class = ?hint.class_name,
                hint_confidence = hint.confidence,
                old_heading_level = ?para.heading_level,
                "Demoting heading: layout model classifies as body text"
            );
            para.heading_level = None;
        }
        _ => {}
    }

    if debug.log_overrides {
        let trimmed = para_text.trim();
        tracing::info!(
            hint_class = ?hint.class_name,
            confidence = hint.confidence,
            old_heading = ?old_heading,
            new_heading = ?para.heading_level,
            font_above_body,
            is_bold = para.is_bold,
            has_strong_heading_evidence,
            words = word_count,
            text = %trimmed.chars().take(60).collect::<String>(),
            "layout override"
        );
    }
}

/// Compute a paragraph's bounding box from its line segments' positional data.
///
/// Returns `None` if the paragraph has no segments with valid positional data.
///
/// In PDF coordinates (y=0 at bottom, y increases upward):
/// - `seg.y` / `seg.baseline_y` is the text baseline (near the bottom of glyphs).
/// - Text extends UPWARD from the baseline by roughly the ascent (~80% of font size).
/// - Text extends DOWNWARD from the baseline by the descent (~20% of font size).
///
/// For layout detection matching, we approximate the visual text extent as:
/// - top = baseline + height (covers ascenders)
/// - bottom = baseline (descent is small and usually within the layout hint's margin)
fn compute_paragraph_bbox(para: &PdfParagraph) -> Option<Rect> {
    if let Some((left, bottom, right, top)) = para.block_bbox
        && right > left
        && top > bottom
    {
        return Some(Rect::from_lbrt(left, bottom, right, top));
    }

    let mut left = f32::MAX;
    let mut right = f32::MIN;
    let mut bottom = f32::MAX;
    let mut top = f32::MIN;
    let mut has_data = false;

    for line in &para.lines {
        for seg in &line.segments {
            if seg.x == 0.0 && seg.width == 0.0 && seg.y == 0.0 && seg.height == 0.0 {
                continue;
            }
            has_data = true;
            left = left.min(seg.x);
            right = right.max(seg.x + seg.width);
            top = top.max(seg.y + seg.height);
            bottom = bottom.min(seg.y);
        }
    }

    if has_data {
        Some(Rect::from_lbrt(left, bottom, right, top))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::hierarchy::SegmentData;
    use crate::pdf::structure::types::PdfLine;

    fn make_segment(text: &str, x: f32, y: f32, width: f32, height: f32) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x,
            y,
            width,
            height,
            font_size: 12.0,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: y,
            assigned_role: None,
        }
    }

    fn make_line_at(segments: Vec<SegmentData>, baseline_y: f32) -> PdfLine {
        PdfLine {
            segments,
            baseline_y,
            dominant_font_size: 12.0,
            is_bold: false,
            is_monospace: false,
        }
    }

    fn make_line(segments: Vec<SegmentData>) -> PdfLine {
        make_line_at(segments, 700.0)
    }

    fn make_para(x: f32, y: f32, width: f32, height: f32) -> PdfParagraph {
        let lines = vec![make_line(vec![make_segment("text", x, y, width, height)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        PdfParagraph {
            text: String::new(),
            lines,
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
            word_count,
        }
    }

    fn make_hint(class: LayoutHintClass, confidence: f32, left: f32, bottom: f32, right: f32, top: f32) -> LayoutHint {
        LayoutHint {
            class_name: class,
            confidence,
            left,
            bottom,
            right,
            top,
        }
    }

    #[test]
    fn test_title_override() {
        let mut paragraphs = vec![make_para(50.0, 750.0, 500.0, 20.0)];
        let hints = vec![make_hint(LayoutHintClass::Title, 0.9, 40.0, 745.0, 560.0, 775.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[0].layout_class, Some(LayoutHintClass::Title));
    }

    #[test]
    fn test_section_header_override() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        let hints = vec![make_hint(
            LayoutHintClass::SectionHeader,
            0.85,
            40.0,
            598.0,
            400.0,
            620.0,
        )];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, Some(2));
    }

    #[test]
    fn test_title_hint_does_not_promote_list_item_to_heading() {
        let mut para = make_para(50.0, 650.0, 300.0, 14.0);
        para.is_list_item = true;
        let mut paragraphs = vec![para];
        let hints = vec![make_hint(LayoutHintClass::Title, 0.9, 40.0, 645.0, 360.0, 670.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(
            paragraphs[0].heading_level, None,
            "Title hint must not promote a list item to H1"
        );
        assert!(
            paragraphs[0].is_list_item,
            "is_list_item must be preserved when Title hint is rejected"
        );
    }

    #[test]
    fn test_section_header_hint_does_not_promote_list_item_to_heading() {
        let mut para = make_para(50.0, 600.0, 300.0, 14.0);
        para.is_list_item = true;
        let mut paragraphs = vec![para];
        let hints = vec![make_hint(
            LayoutHintClass::SectionHeader,
            0.9,
            40.0,
            595.0,
            360.0,
            620.0,
        )];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(
            paragraphs[0].heading_level, None,
            "SectionHeader hint must not promote a list item to a heading"
        );
        assert!(
            paragraphs[0].is_list_item,
            "is_list_item must be preserved when SectionHeader hint is rejected"
        );
    }

    #[test]
    fn test_low_confidence_ignored() {
        let mut paragraphs = vec![make_para(50.0, 750.0, 500.0, 20.0)];
        let hints = vec![make_hint(LayoutHintClass::Title, 0.3, 40.0, 745.0, 560.0, 775.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, None);
        assert_eq!(paragraphs[0].layout_class, None);
    }

    #[test]
    fn test_existing_heading_overridden_by_high_confidence() {
        let mut paragraphs = vec![make_para(50.0, 750.0, 500.0, 20.0)];
        paragraphs[0].heading_level = Some(3);
        let hints = vec![make_hint(
            LayoutHintClass::SectionHeader,
            0.9,
            40.0,
            745.0,
            560.0,
            775.0,
        )];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, Some(2));
    }

    #[test]
    fn test_existing_heading_preserved_low_confidence() {
        let mut paragraphs = vec![make_para(50.0, 750.0, 500.0, 20.0)];
        paragraphs[0].heading_level = Some(3);
        let hints = vec![make_hint(
            LayoutHintClass::SectionHeader,
            0.6,
            40.0,
            745.0,
            560.0,
            775.0,
        )];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, Some(3));
    }

    #[test]
    fn test_empty_hints() {
        let mut paragraphs = vec![make_para(50.0, 750.0, 500.0, 20.0)];
        apply_layout_overrides(&mut paragraphs, &[], 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn test_intersection_over_self_full() {
        let hint = Rect::from_lbrt(0.0, 0.0, 612.0, 792.0);
        let para = Rect::from_lbrt(50.0, 100.0, 550.0, 200.0);
        let containment = para.intersection_over_self(&hint);
        assert!(
            (containment - 1.0).abs() < 0.01,
            "Full containment expected: {}",
            containment
        );
    }

    #[test]
    fn test_intersection_over_self_none() {
        let hint = Rect::from_lbrt(0.0, 500.0, 100.0, 600.0);
        let para = Rect::from_lbrt(200.0, 100.0, 500.0, 200.0);
        let containment = para.intersection_over_self(&hint);
        assert!(
            (containment - 0.0).abs() < 0.01,
            "No containment expected: {}",
            containment
        );
    }

    #[test]
    fn test_infer_heading_level_title() {
        assert_eq!(
            infer_heading_level_from_text("Docling Report", LayoutHintClass::Title),
            1
        );
    }

    #[test]
    fn test_infer_heading_level_top_section() {
        assert_eq!(
            infer_heading_level_from_text("3 Processing pipeline", LayoutHintClass::SectionHeader),
            2
        );
    }

    #[test]
    fn test_infer_heading_level_subsection() {
        assert_eq!(
            infer_heading_level_from_text("3.2 AI models", LayoutHintClass::SectionHeader),
            3
        );
    }

    #[test]
    fn test_infer_heading_level_subsubsection() {
        assert_eq!(
            infer_heading_level_from_text("3.2.1 Details", LayoutHintClass::SectionHeader),
            4
        );
    }

    #[test]
    fn test_infer_heading_level_trailing_dot() {
        assert_eq!(
            infer_heading_level_from_text("3. Processing", LayoutHintClass::SectionHeader),
            2
        );
    }

    #[test]
    fn test_infer_heading_level_no_number() {
        assert_eq!(
            infer_heading_level_from_text("Layout Analysis Model", LayoutHintClass::SectionHeader),
            2
        );
    }

    #[test]
    fn test_no_positional_data_skips_layout_overrides() {
        let lines = vec![make_line(vec![make_segment("text", 0.0, 0.0, 0.0, 0.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        let mut paragraphs = vec![PdfParagraph {
            text: String::new(),
            lines,
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
            word_count,
        }];

        let hints = vec![make_hint(LayoutHintClass::Title, 0.9, 40.0, 0.0, 560.0, 760.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, None);
        assert_eq!(paragraphs[0].layout_class, None);

        paragraphs[0].heading_level = None;
        paragraphs[0].layout_class = None;

        let hints = vec![make_hint(LayoutHintClass::PageHeader, 0.9, 40.0, 0.0, 560.0, 760.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(!paragraphs[0].is_page_furniture);
        assert_eq!(paragraphs[0].layout_class, None);
    }

    #[test]
    fn test_separator_pure_dashes() {
        assert!(is_separator_text("----------"));
    }

    #[test]
    fn test_separator_underscores() {
        assert!(is_separator_text("___________"));
    }

    #[test]
    fn test_separator_mixed_with_few_alnum() {
        assert!(is_separator_text("------- M ---------"));
    }

    #[test]
    fn test_separator_empty_string() {
        assert!(!is_separator_text(""));
        assert!(!is_separator_text("   "));
    }

    #[test]
    fn test_separator_normal_text() {
        assert!(!is_separator_text("Hello World"));
    }

    #[test]
    fn test_separator_short_symbols() {
        assert!(is_separator_text("---"));
    }

    #[test]
    fn test_infer_heading_level_alpha_prefix() {
        assert_eq!(
            infer_heading_level_from_text("A. Proofs", LayoutHintClass::SectionHeader),
            2
        );
    }

    #[test]
    fn test_infer_heading_level_alpha_subsection() {
        assert_eq!(
            infer_heading_level_from_text("A.1 Details", LayoutHintClass::SectionHeader),
            3
        );
    }

    #[test]
    fn test_infer_heading_level_deep_subsection() {
        assert_eq!(
            infer_heading_level_from_text("1.2.3.4 Very deep", LayoutHintClass::SectionHeader),
            4
        );
    }

    #[test]
    fn test_code_override() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        let hints = vec![make_hint(LayoutHintClass::Code, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn test_code_override_rejects_prose() {
        let mut para = make_para(50.0, 600.0, 300.0, 16.0);
        para.text = "Duis autem vel eum iriure dolor in hendrerit in vulputate velit esse molestie consequat. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore.".to_string();
        let mut paragraphs = vec![para];
        let hints = vec![make_hint(LayoutHintClass::Code, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(
            !paragraphs[0].is_code_block,
            "Prose text should not be classified as code"
        );
    }

    #[test]
    fn test_code_override_accepts_real_code() {
        let mut para = make_para(50.0, 600.0, 300.0, 16.0);
        para.text = "function add(a, b) { return a + b; }".to_string();
        let mut paragraphs = vec![para];
        let hints = vec![make_hint(LayoutHintClass::Code, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(
            paragraphs[0].is_code_block,
            "Code-like text should be classified as code"
        );
    }

    #[test]
    fn test_code_override_preserves_strong_heading_below_confidence_threshold() {
        for heading in ["Recent Change Log", "C# API", "API (v2)", "Results (n = 10)"] {
            let mut para = make_para(50.0, 600.0, 300.0, 16.0);
            para.text = heading.to_string();
            para.heading_level = Some(2);
            para.is_bold = true;
            let mut paragraphs = vec![para];
            let hints = [make_hint(LayoutHintClass::Code, 0.751_801_5, 40.0, 598.0, 400.0, 620.0)];

            apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

            assert!(!paragraphs[0].is_code_block, "{heading}");
            assert_eq!(paragraphs[0].heading_level, Some(2), "{heading}");
        }
    }

    #[test]
    fn test_code_override_accepts_structured_code_despite_strong_heading_evidence() {
        let mut para = make_para(50.0, 600.0, 300.0, 16.0);
        para.text = "function add(a, b) { return a + b; }".to_string();
        para.heading_level = Some(2);
        para.is_bold = true;
        let mut paragraphs = vec![para];
        let hints = [make_hint(LayoutHintClass::Code, 0.751_801_5, 40.0, 598.0, 400.0, 620.0)];

        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn test_code_override_accepts_bold_code_without_native_heading() {
        let mut para = make_para(50.0, 600.0, 300.0, 16.0);
        para.text = "SELECT ID FROM USERS".to_string();
        para.is_bold = true;
        let mut paragraphs = vec![para];
        let hints = [make_hint(LayoutHintClass::Code, 0.751_801_5, 40.0, 598.0, 400.0, 620.0)];

        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn test_formula_override() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].lines[0].segments[0].text = "E = mc^2".to_string();
        let hints = vec![make_hint(LayoutHintClass::Formula, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(paragraphs[0].is_formula);
    }

    #[test]
    fn test_list_item_override() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].text = "• Item one".to_string();
        let hints = vec![make_hint(LayoutHintClass::ListItem, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(paragraphs[0].is_list_item);
    }

    #[test]
    fn test_body_text_demotes_heading() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        let hints = vec![make_hint(LayoutHintClass::Text, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn test_body_text_low_confidence_preserves_heading() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        let hints = vec![make_hint(LayoutHintClass::Text, 0.6, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].heading_level, Some(2));
    }

    #[test]
    fn test_bold_heading_not_demoted_by_high_confidence_text_hint() {
        // A2: a bold paragraph carries independent heading evidence, so even a
        // high-confidence Text hint must not erase its heading level. ~keep
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        paragraphs[0].is_bold = true;
        let hints = vec![make_hint(LayoutHintClass::Text, 0.95, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(
            paragraphs[0].heading_level,
            Some(2),
            "bold heading must survive a high-confidence Text demotion hint"
        );
    }

    #[test]
    fn test_large_font_heading_not_demoted_by_text_hint() {
        // A2: font clearly above body size is independent heading evidence. ~keep
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        paragraphs[0].dominant_font_size = 16.0;
        let hints = vec![make_hint(LayoutHintClass::Text, 0.95, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, Some(10.0));
        assert_eq!(
            paragraphs[0].heading_level,
            Some(2),
            "font-above-body heading must survive a Text demotion hint"
        );
    }

    #[test]
    fn test_body_text_borderline_confidence_preserves_heading() {
        // A2: demotion now requires confidence >= HEADING_DEMOTE_CONFIDENCE (0.85),
        // above the old 0.7 bar. A 0.8 hint no longer erases a heading. ~keep
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        let hints = vec![make_hint(LayoutHintClass::Text, 0.8, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(
            paragraphs[0].heading_level,
            Some(2),
            "0.8-confidence Text hint is below the demote threshold and must preserve the heading"
        );
    }

    #[test]
    fn test_body_font_false_heading_still_demotes() {
        // A2 must not over-suppress: a body-font, non-bold, non-numbered false heading
        // with a high-confidence Text hint still demotes (no independent evidence). ~keep
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        paragraphs[0].heading_level = Some(2);
        let hints = vec![make_hint(LayoutHintClass::Text, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, Some(12.0));
        assert_eq!(
            paragraphs[0].heading_level, None,
            "evidence-free false heading must still demote under a high-confidence Text hint"
        );
    }

    #[test]
    fn test_page_footer_override() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        let hints = vec![make_hint(LayoutHintClass::PageFooter, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert!(paragraphs[0].is_page_furniture);
    }

    #[test]
    fn test_separator_text_not_promoted_to_heading() {
        let lines = vec![make_line(vec![make_segment("----------", 50.0, 600.0, 300.0, 16.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        let mut para = PdfParagraph {
            text: String::new(),
            lines,
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
            word_count,
        };
        let hint = make_hint(LayoutHintClass::SectionHeader, 0.9, 40.0, 598.0, 400.0, 620.0);
        apply_hint_to_paragraph(&mut para, &hint, None);
        assert_eq!(para.heading_level, None);
    }

    #[test]
    fn test_compute_paragraph_bbox_no_positional_data() {
        let lines = vec![make_line(vec![make_segment("text", 0.0, 0.0, 0.0, 0.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        let para = PdfParagraph {
            text: String::new(),
            lines,
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
            word_count,
        };
        assert!(compute_paragraph_bbox(&para).is_none());
    }

    #[test]
    fn test_compute_paragraph_bbox_with_block_bbox() {
        let lines = vec![make_line(vec![make_segment("text", 0.0, 0.0, 0.0, 0.0)])];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        let para = PdfParagraph {
            text: String::new(),
            lines,
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
            block_bbox: Some((50.0, 100.0, 400.0, 120.0)),
            word_count,
        };
        let bbox = compute_paragraph_bbox(&para).unwrap();
        assert!((bbox.left - 50.0).abs() < f32::EPSILON);
        assert!((bbox.y_min - 100.0).abs() < f32::EPSILON);
        assert!((bbox.right - 400.0).abs() < f32::EPSILON);
        assert!((bbox.y_max - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compute_paragraph_bbox_from_segments() {
        let lines = vec![
            make_line_at(vec![make_segment("A", 50.0, 700.0, 100.0, 12.0)], 700.0),
            make_line_at(vec![make_segment("B", 60.0, 680.0, 120.0, 14.0)], 680.0),
        ];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        let para = PdfParagraph {
            text: String::new(),
            lines,
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
            word_count,
        };
        let bbox = compute_paragraph_bbox(&para).unwrap();
        assert!((bbox.left - 50.0).abs() < f32::EPSILON);
        assert!((bbox.y_min - 680.0).abs() < f32::EPSILON);
        assert!((bbox.right - 180.0).abs() < f32::EPSILON);
        assert!((bbox.y_max - 712.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_page_header_furniture_requires_high_confidence() {
        let mut para = PdfParagraph {
            text: "Header text with content".to_string(),
            lines: vec![],
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
            word_count: 5,
        };

        let low_conf_hint = LayoutHint {
            class_name: LayoutHintClass::PageHeader,
            confidence: 0.7,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &low_conf_hint, None);
        assert!(
            !para.is_page_furniture,
            "Low-confidence PageHeader (0.7) should NOT mark paragraph as furniture"
        );
        assert_eq!(
            para.layout_class,
            Some(LayoutHintClass::PageHeader),
            "layout_class should still be set"
        );

        let mut para2 = PdfParagraph {
            text: "Header text with content".to_string(),
            lines: vec![],
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
            word_count: 5,
        };
        let high_conf_hint = LayoutHint {
            class_name: LayoutHintClass::PageHeader,
            confidence: 0.85,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para2, &high_conf_hint, None);
        assert!(
            para2.is_page_furniture,
            "High-confidence PageHeader (0.85) should mark paragraph as furniture"
        );
    }

    #[test]
    fn test_page_footer_furniture_requires_high_confidence() {
        let mut para = PdfParagraph {
            text: "Footer text".to_string(),
            lines: vec![],
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
            word_count: 2,
        };

        let low_conf_hint = LayoutHint {
            class_name: LayoutHintClass::PageFooter,
            confidence: 0.6,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &low_conf_hint, None);
        assert!(
            !para.is_page_furniture,
            "Low-confidence PageFooter (0.6) should NOT mark paragraph as furniture"
        );
    }

    #[test]
    fn test_code_hint_does_not_override_native_list_item() {
        let mut para = PdfParagraph {
            text: "· Explain the importance of asking questions.".to_string(),
            lines: vec![],
            dominant_font_size: 12.0,
            heading_level: None,
            is_bold: false,
            is_list_item: true,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count: 7,
        };

        let hint = LayoutHint {
            class_name: LayoutHintClass::Code,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &hint, None);

        assert!(
            !para.is_code_block,
            "Code hint must not override a natively classified list item"
        );
        assert!(
            para.is_list_item,
            "List item flag must be preserved when Code hint is rejected"
        );
        assert_eq!(
            para.heading_level, None,
            "heading_level must remain None (list items have no heading level)"
        );
    }

    #[test]
    fn test_code_hint_applies_to_non_list_item_paragraph() {
        let mut para = PdfParagraph {
            text: "function add(a, b) { return a + b; }".to_string(),
            lines: vec![],
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
            word_count: 8,
        };

        let hint = LayoutHint {
            class_name: LayoutHintClass::Code,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &hint, None);

        assert!(para.is_code_block, "Code hint must apply to non-list-item paragraphs");
        assert!(!para.is_list_item, "is_list_item must remain false");
    }

    #[test]
    fn test_page_header_hint_does_not_suppress_native_heading() {
        let mut para = PdfParagraph {
            text: "Sample PDF".to_string(),
            lines: vec![],
            dominant_font_size: 24.0,
            heading_level: Some(1),
            is_bold: true,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count: 2,
        };

        let hint = LayoutHint {
            class_name: LayoutHintClass::PageHeader,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &hint, None);

        assert!(
            !para.is_page_furniture,
            "High-confidence PageHeader hint must not suppress a natively classified H1"
        );
        assert_eq!(
            para.heading_level,
            Some(1),
            "heading_level must be preserved when PageHeader hint is rejected for headings"
        );
    }

    #[test]
    fn test_page_header_hint_applies_to_non_heading_paragraph() {
        let mut para = PdfParagraph {
            text: "Page 5".to_string(),
            lines: vec![],
            dominant_font_size: 10.0,
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
            word_count: 2,
        };

        let hint = LayoutHint {
            class_name: LayoutHintClass::PageHeader,
            confidence: 0.9,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &hint, None);

        assert!(
            para.is_page_furniture,
            "High-confidence PageHeader hint must still mark non-heading paragraphs as furniture"
        );
    }

    #[test]
    fn test_page_footer_hint_does_not_suppress_native_heading() {
        let mut para = PdfParagraph {
            text: "Conclusions".to_string(),
            lines: vec![],
            dominant_font_size: 16.0,
            heading_level: Some(2),
            is_bold: true,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count: 1,
        };

        let hint = LayoutHint {
            class_name: LayoutHintClass::PageFooter,
            confidence: 0.95,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &hint, None);

        assert!(
            !para.is_page_furniture,
            "PageFooter hint must not suppress a natively classified H2"
        );
        assert_eq!(para.heading_level, Some(2));
    }

    #[test]
    fn test_native_paragraph_table_hint_passes_through() {
        let mut paragraphs = vec![make_para(50.0, 600.0, 300.0, 16.0)];
        let hints = vec![make_hint(LayoutHintClass::Table, 0.9, 40.0, 598.0, 400.0, 620.0)];
        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);
        assert_eq!(paragraphs[0].layout_class, Some(LayoutHintClass::Table));
        assert_eq!(paragraphs[0].heading_level, None);
        assert!(!paragraphs[0].is_list_item);
        assert!(!paragraphs[0].is_code_block);
        assert!(!paragraphs[0].is_page_furniture);
    }

    #[test]
    fn test_picture_hint_does_not_suppress_native_heading() {
        let lines = vec![make_line(vec![make_segment("Sample PDF", 0.0, 800.0, 200.0, 36.0)])];
        let word_count = PdfParagraph::compute_word_count("Sample PDF", &lines);
        let mut para = PdfParagraph {
            text: "Sample PDF".to_string(),
            lines,
            dominant_font_size: 36.0,
            heading_level: Some(1),
            is_bold: false,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count,
        };
        let hint = make_hint(LayoutHintClass::Picture, 0.72, 0.0, 790.0, 250.0, 820.0);
        apply_hint_to_paragraph(&mut para, &hint, None);
        assert_eq!(para.heading_level, Some(1), "H1 must survive Picture hint");
        assert!(
            !para.is_page_furniture,
            "furniture must not be set when heading is present"
        );
    }

    #[test]
    fn test_picture_hint_applies_to_non_heading_paragraph() {
        let mut para = make_para(12.0, 400.0, 100.0, 16.0);
        para.text = "Figure 1: schematic".to_string();
        let hint = make_hint(LayoutHintClass::Picture, 0.85, 0.0, 390.0, 200.0, 420.0);
        apply_hint_to_paragraph(&mut para, &hint, None);
        assert!(para.is_page_furniture, "figure label must become furniture");
        assert_eq!(para.heading_level, None, "non-heading para must stay non-heading");
    }

    #[test]
    fn test_list_item_requires_high_confidence() {
        let mut para = PdfParagraph {
            text: "1. First item in list".to_string(),
            lines: vec![],
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
            word_count: 4,
        };

        let low_conf_hint = LayoutHint {
            class_name: LayoutHintClass::ListItem,
            confidence: 0.7,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para, &low_conf_hint, None);
        assert!(
            !para.is_list_item,
            "Low-confidence ListItem (0.7) should NOT mark paragraph as list item"
        );

        let mut para2 = PdfParagraph {
            text: "1. First item in list".to_string(),
            lines: vec![],
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
            word_count: 4,
        };
        let high_conf_hint = LayoutHint {
            class_name: LayoutHintClass::ListItem,
            confidence: 0.85,
            left: 0.0,
            bottom: 0.0,
            right: 100.0,
            top: 50.0,
        };
        apply_hint_to_paragraph(&mut para2, &high_conf_hint, None);
        assert!(
            para2.is_list_item,
            "High-confidence ListItem (0.85) should mark paragraph as list item"
        );
    }

    #[test]
    fn semantic_hint_beats_broad_text_when_containment_is_comparable() {
        let mut paragraphs = vec![make_para(10.0, 100.0, 100.0, 10.0)];
        let hints = vec![
            make_hint(LayoutHintClass::Text, 0.95, 10.0, 100.0, 110.0, 110.0),
            make_hint(LayoutHintClass::SectionHeader, 0.90, 10.0, 100.0, 106.0, 110.0),
        ];

        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

        assert_eq!(paragraphs[0].layout_class, Some(LayoutHintClass::SectionHeader));
        assert_eq!(paragraphs[0].heading_level, Some(2));
    }

    #[test]
    fn materially_better_text_containment_is_preserved() {
        let mut paragraphs = vec![make_para(10.0, 100.0, 100.0, 10.0)];
        let hints = vec![
            make_hint(LayoutHintClass::Text, 0.90, 10.0, 100.0, 110.0, 110.0),
            make_hint(LayoutHintClass::SectionHeader, 0.99, 10.0, 100.0, 100.0, 110.0),
        ];

        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

        assert_eq!(paragraphs[0].layout_class, Some(LayoutHintClass::Text));
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[test]
    fn unvalidated_formula_hint_does_not_beat_comparable_text() {
        let mut paragraphs = vec![make_para(10.0, 100.0, 100.0, 10.0)];
        let hints = vec![
            make_hint(LayoutHintClass::Text, 0.90, 10.0, 100.0, 110.0, 110.0),
            make_hint(LayoutHintClass::Formula, 0.99, 10.0, 100.0, 106.0, 110.0),
        ];

        apply_layout_overrides(&mut paragraphs, &hints, 0.5, 0.5, None);

        assert_eq!(paragraphs[0].layout_class, Some(LayoutHintClass::Text));
        assert!(!paragraphs[0].is_formula);
    }

    #[test]
    fn equivalent_semantic_hints_are_selected_independently_of_input_order() {
        let title = make_hint(LayoutHintClass::Title, 0.90, 10.0, 100.0, 110.0, 110.0);
        let section = make_hint(LayoutHintClass::SectionHeader, 0.90, 10.0, 100.0, 110.0, 110.0);
        let mut forward = vec![make_para(10.0, 100.0, 100.0, 10.0)];
        let mut reverse = forward.clone();

        apply_layout_overrides(&mut forward, &[title.clone(), section.clone()], 0.5, 0.5, None);
        apply_layout_overrides(&mut reverse, &[section, title], 0.5, 0.5, None);

        assert_eq!(forward[0].layout_class, reverse[0].layout_class);
        assert_eq!(forward[0].heading_level, reverse[0].heading_level);
    }
}
