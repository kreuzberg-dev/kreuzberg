//! Utilities for splitting and analyzing PDF paragraphs.

use super::types::PdfParagraph;

/// Maximum baseline-to-baseline gap, as a multiple of the larger paragraph's
/// dominant font size, permitted when merging a continuation paragraph.
///
/// A wrapped continuation line sits roughly one line-height (leading is
/// typically 1.0–1.6× the font size) below its predecessor. A gap several times
/// larger means the two paragraphs come from spatially distinct regions — e.g. a
/// recipient block near the top of an invoice and a legal footer near the
/// bottom — and must never be joined into one logical paragraph, which would
/// associate their text (and mangle the merged block's bounding box). See #1350.
const MAX_CONTINUATION_LINE_GAP_MULTIPLE: f32 = 3.0;

/// Merge consecutive body-text paragraphs that are continuations of the same logical paragraph.
///
/// Two consecutive paragraphs are merged if:
/// - Both are body text (no heading_level, not is_list_item)
/// - The first paragraph doesn't end with sentence-ending punctuation
/// - Font sizes are within 2pt of each other
/// - Their baselines are within [`MAX_CONTINUATION_LINE_GAP_MULTIPLE`] line-heights
pub(super) fn merge_continuation_paragraphs(paragraphs: &mut Vec<PdfParagraph>) {
    if paragraphs.len() < 2 {
        return;
    }

    let old = std::mem::take(paragraphs);
    let mut iter = old.into_iter();
    let mut current = iter.next().unwrap();

    for next in iter {
        let both_body = current.heading_level.is_none()
            && next.heading_level.is_none()
            && !current.is_list_item
            && !next.is_list_item
            && !current.is_code_block
            && !next.is_code_block
            && !current.is_formula
            && !next.is_formula;
        let fonts_compatible = (current.dominant_font_size - next.dominant_font_size).abs() < 2.0;
        // Never merge across a bold-state boundary. A bold run following non-bold
        // prose (or vice versa) is a formatting break — an emphasized heading or a
        // list item's bold lead-in — not a wrapped continuation. Absorbing it would
        // bury the heading as inline bold before it can be classified. This mirrors
        // the `bold_change` paragraph break in the heuristic line grouper. ~keep
        let bold_compatible = current.is_bold == next.is_bold;
        let continuation_signal = !ends_with_sentence_terminator(&current) || starts_with_lowercase_continuation(&next);
        let same_region = current.layout_region_path == next.layout_region_path;
        let vertical_gap_compatible = baselines_within_continuation_gap(&current, &next);
        // A numbered section heading starts a new logical element and must never be
        // absorbed as a continuation. A heading does not end in `.?!:;`, so
        // `continuation_signal` is satisfied by the *previous* heading alone — it is
        // an `||` boost, not a requirement — and a run of consecutive subsection
        // headings would otherwise be re-joined here even after the line grouper
        // split them. See #1386. ~keep
        let next_starts_section = starts_numbered_section(&next);
        let should_merge = both_body
            && fonts_compatible
            && bold_compatible
            && continuation_signal
            && same_region
            && vertical_gap_compatible
            && !next_starts_section;

        if should_merge {
            current.text.clear();
            current.block_bbox = union_block_bbox(current.block_bbox, next.block_bbox);
            current.lines.extend(next.lines);
        } else {
            paragraphs.push(current);
            current = next;
        }
    }

    paragraphs.push(current);
}

/// Whether `next`'s first baseline is close enough below `current`'s last
/// baseline to be a wrapped continuation rather than a spatially distant block.
///
/// Returns `true` when either paragraph lacks per-line geometry (`baseline_y`
/// unset on the structure-tree path), preserving the prior behavior for inputs
/// where a vertical distance cannot be computed.
fn baselines_within_continuation_gap(current: &PdfParagraph, next: &PdfParagraph) -> bool {
    let (Some(current_last), Some(next_first)) = (current.lines.last(), next.lines.first()) else {
        return true;
    };
    // No geometry available (e.g. synthesized paragraphs): fall back to allowing
    // the merge, gated by the other continuation signals.
    if current_last.baseline_y == 0.0 || next_first.baseline_y == 0.0 {
        return true;
    }
    let gap = (current_last.baseline_y - next_first.baseline_y).abs();
    let line_height = current.dominant_font_size.max(next.dominant_font_size).max(1.0);
    gap <= line_height * MAX_CONTINUATION_LINE_GAP_MULTIPLE
}

/// Union of two optional block bounding boxes in `(left, bottom, right, top)`
/// PDF-coordinate form, so a merged paragraph's box spans all of its text.
fn union_block_bbox(
    current: Option<(f32, f32, f32, f32)>,
    next: Option<(f32, f32, f32, f32)>,
) -> Option<(f32, f32, f32, f32)> {
    match (current, next) {
        (Some((cl, cb, cr, ct)), Some((nl, nb, nr, nt))) => Some((cl.min(nl), cb.min(nb), cr.max(nr), ct.max(nt))),
        (Some(bbox), None) | (None, Some(bbox)) => Some(bbox),
        (None, None) => None,
    }
}

/// Whether a paragraph's first visual line reads as a numbered section heading
/// ("1.3 Gasinstallatie", "IV. Results", "1. INTRODUCTION").
///
/// The first line's segments are re-joined because word-processor output often
/// splits the numbering into its own text run, so the leading segment alone can
/// be a bare "1.3". The conservative `is_numbered_section_heading` is used rather
/// than `starts_with_section_number` so a paragraph opening with a bare year
/// ("2024 was een druk jaar") is still treated as prose.
fn starts_numbered_section(para: &PdfParagraph) -> bool {
    let Some(first_line) = para.lines.first() else {
        return false;
    };
    let joined = first_line
        .segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    super::classify::is_numbered_section_heading(&joined)
}

/// Check if a paragraph starts with a lowercase letter, indicating it's a
/// continuation of a previous sentence split across paragraph boundaries.
fn starts_with_lowercase_continuation(para: &PdfParagraph) -> bool {
    let first_text = para
        .lines
        .first()
        .and_then(|l| l.segments.first())
        .map(|s| s.text.trim_start())
        .unwrap_or("");
    first_text.chars().next().is_some_and(|c| c.is_lowercase())
}

/// Check if a paragraph's last line ends with sentence-terminating punctuation.
///
/// Used by merge_continuation_paragraphs to determine if two consecutive
/// paragraphs should be merged. Supports ASCII and CJK sentence terminators.
fn ends_with_sentence_terminator(para: &PdfParagraph) -> bool {
    let last_text = para
        .lines
        .last()
        .and_then(|l| l.segments.last())
        .map(|s| s.text.trim_end())
        .unwrap_or("");
    matches!(
        last_text.chars().last(),
        Some('.' | '?' | '!' | ':' | ';' | '\u{3002}' | '\u{FF1F}' | '\u{FF01}')
    )
}

/// Split paragraphs that contain embedded bullet characters (e.g. `•`) into separate list items.
///
/// Structure tree pages sometimes merge all text into one block with inline bullets.
/// This splits "text before • item1 • item2" into separate paragraphs.
pub(super) fn split_embedded_list_items(paragraphs: &mut Vec<PdfParagraph>) {
    let old = std::mem::take(paragraphs);
    for para in old {
        if para.heading_level.is_some() || para.is_list_item || para.is_code_block || para.is_formula {
            paragraphs.push(para);
            continue;
        }

        let full_text: String = para
            .lines
            .iter()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let bullet_count = full_text.matches(['\u{2022}', '\u{00B7}']).count();
        if bullet_count < 2 {
            paragraphs.push(para);
            continue;
        }

        let font_size = para.dominant_font_size;
        let is_bold = para.is_bold;

        let parts: Vec<&str> = full_text.split(['\u{2022}', '\u{00B7}']).collect();
        let before = parts[0].trim().trim_end_matches('\u{00C2}').trim();
        if !before.is_empty() {
            paragraphs.push(text_to_paragraph(before, font_size, is_bold, false));
        }
        for part in &parts[1..] {
            let item_text = part
                .trim()
                .trim_start_matches('\u{00C2}')
                .trim_end_matches('\u{00C2}')
                .trim();
            if !item_text.is_empty() {
                paragraphs.push(text_to_paragraph(item_text, font_size, is_bold, true));
            }
        }
    }
}

/// Create a simple paragraph from text.
fn text_to_paragraph(text: &str, font_size: f32, is_bold: bool, is_list_item: bool) -> PdfParagraph {
    use crate::pdf::hierarchy::SegmentData;

    let segments: Vec<SegmentData> = text
        .split_whitespace()
        .map(|w| SegmentData {
            text: w.to_string(),
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            font_size,
            is_bold,
            is_italic: false,
            is_monospace: false,
            baseline_y: 0.0,
            assigned_role: None,
        })
        .collect();

    let line = super::types::PdfLine {
        segments,
        baseline_y: 0.0,
        dominant_font_size: font_size,
        is_bold,
        is_monospace: false,
    };

    let lines = vec![line];
    let word_count = PdfParagraph::compute_word_count("", &lines);
    PdfParagraph {
        text: String::new(),
        lines,
        dominant_font_size: font_size,
        heading_level: None,
        is_bold,
        is_list_item,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_body_paragraph(text: &str, font_size: f32) -> PdfParagraph {
        use crate::pdf::hierarchy::SegmentData;

        let segments = vec![SegmentData {
            text: text.to_string(),
            x: 0.0,
            y: 700.0,
            width: 200.0,
            height: font_size,
            font_size,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: 700.0,
            assigned_role: None,
        }];

        let lines = vec![super::super::types::PdfLine {
            segments,
            baseline_y: 700.0,
            dominant_font_size: font_size,
            is_bold: false,
            is_monospace: false,
        }];
        let word_count = PdfParagraph::compute_word_count("", &lines);
        PdfParagraph {
            text: String::new(),
            lines,
            dominant_font_size: font_size,
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

    fn make_body_paragraph_at(text: &str, font_size: f32, baseline_y: f32) -> PdfParagraph {
        let mut para = make_body_paragraph(text, font_size);
        para.lines[0].baseline_y = baseline_y;
        if let Some(segment) = para.lines[0].segments.first_mut() {
            segment.baseline_y = baseline_y;
            segment.y = baseline_y;
        }
        para
    }

    #[test]
    fn test_no_merge_across_distant_regions() {
        // A buyer tax ID in the upper recipient block and a seller tax ID in the
        // page footer are ~590pt apart. Neither ends with a sentence terminator,
        // so the older heuristic would merge them; the vertical-gap guard must
        // keep them separate. Regression for #1350.
        let mut paragraphs = vec![
            make_body_paragraph_at("Buyer tax ID SYNTH-BUYER-TAX-359370919", 8.0, 185.5),
            make_body_paragraph_at("Seller tax ID SYNTH-SELLER-TAX-815876165", 8.0, 775.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(
            paragraphs.len(),
            2,
            "paragraphs from spatially distant regions must not merge"
        );
    }

    #[test]
    fn test_merge_adjacent_lines_within_gap() {
        // Genuine wrapped continuation one line-height apart still merges.
        let mut paragraphs = vec![
            make_body_paragraph_at("The committee reviewed the annual", 11.0, 712.0),
            make_body_paragraph_at("report and approved the budget", 11.0, 698.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1, "adjacent continuation lines should merge");
    }

    #[test]
    fn test_merge_unions_block_bbox() {
        let mut upper = make_body_paragraph_at("first line without terminator", 11.0, 712.0);
        upper.block_bbox = Some((60.0, 705.0, 260.0, 720.0));
        let mut lower = make_body_paragraph_at("second line continues", 11.0, 698.0);
        lower.block_bbox = Some((60.0, 691.0, 300.0, 706.0));
        let mut paragraphs = vec![upper, lower];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1, "adjacent lines should merge");
        assert_eq!(
            paragraphs[0].block_bbox,
            Some((60.0, 691.0, 300.0, 720.0)),
            "merged block bbox must span both source boxes"
        );
    }

    #[test]
    fn test_merge_lowercase_continuation() {
        let mut paragraphs = vec![
            make_body_paragraph("The regulation requires.", 12.0),
            make_body_paragraph("and all operators must comply", 12.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1, "lowercase continuation should be merged");
    }

    #[test]
    fn test_no_merge_different_font_sizes() {
        let mut paragraphs = vec![
            make_body_paragraph("First paragraph", 12.0),
            make_body_paragraph("second paragraph", 16.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 2, "different font sizes should prevent merge");
    }

    #[test]
    fn test_merge_no_terminator() {
        let mut paragraphs = vec![
            make_body_paragraph("The regulation requires", 12.0),
            make_body_paragraph("All operators must comply", 12.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1, "unterminated paragraph should merge with next");
    }

    #[test]
    fn test_no_merge_terminated_uppercase() {
        let mut paragraphs = vec![
            make_body_paragraph("The regulation requires compliance.", 12.0),
            make_body_paragraph("All operators must comply", 12.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(
            paragraphs.len(),
            2,
            "terminated paragraph + uppercase start should not merge"
        );
    }

    #[test]
    fn test_no_merge_across_bold_boundary() {
        // A bold header following unterminated body prose must not be absorbed as
        // a continuation — it should survive as its own paragraph for classification. ~keep
        let body = make_body_paragraph(
            "here is also available other sources of this Manual MetcalUser Guide",
            12.0,
        );
        let mut header = make_body_paragraph("Impaired Glucose Tolerance And Impaired Fasting Glucose ...", 12.0);
        header.is_bold = true;
        let mut paragraphs = vec![body, header];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 2, "bold header must not merge into non-bold prose");
        assert!(paragraphs[1].is_bold, "the bold header paragraph must be preserved");
    }

    /// Text of a paragraph's first line, joined from its segments.
    fn first_line_text(para: &PdfParagraph) -> String {
        para.lines
            .first()
            .map(|line| {
                line.segments
                    .iter()
                    .map(|s| s.text.trim())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }

    /// Regression for #1386 (defect #290): a run of consecutive numbered
    /// subsection headings at the same size and weight, one line-height apart,
    /// must survive as four separate paragraphs. None of them ends in `.?!:;`,
    /// so `continuation_signal` alone would merge the whole run into one element
    /// and a consumer deriving a section hierarchy would see only "1.3".
    #[test]
    fn should_not_merge_consecutive_numbered_section_headings() {
        let mut paragraphs = vec![
            make_body_paragraph_at("1.3 Gasinstallatie", 11.0, 700.0),
            make_body_paragraph_at("1.4 Elektrische installatie", 11.0, 686.0),
            make_body_paragraph_at("1.5 Waterinstallatie", 11.0, 672.0),
            make_body_paragraph_at("1.6 Ventilatie", 11.0, 658.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            4,
            "each numbered subsection heading must remain a separate element"
        );
        assert_eq!(first_line_text(&paragraphs[0]), "1.3 Gasinstallatie");
        assert_eq!(first_line_text(&paragraphs[1]), "1.4 Elektrische installatie");
        assert_eq!(first_line_text(&paragraphs[2]), "1.5 Waterinstallatie");
        assert_eq!(first_line_text(&paragraphs[3]), "1.6 Ventilatie");
    }

    /// The over-fire guard for #1386: prose whose first token is a bare year is
    /// NOT a section heading and must still merge with the unterminated line
    /// above it. `starts_with_section_number` returns `true` here — using it
    /// instead of `is_numbered_section_heading` would split ordinary prose.
    #[test]
    fn should_merge_prose_paragraph_starting_with_a_year() {
        let mut paragraphs = vec![
            make_body_paragraph_at("Het bestuur meldt", 11.0, 700.0),
            make_body_paragraph_at("2024 was een druk jaar", 11.0, 686.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            1,
            "prose beginning with a bare year is not a section heading and must stay one paragraph"
        );
        assert_eq!(paragraphs[0].lines.len(), 2, "both prose lines must be present");
    }

    /// Roman-numeral and single-level ALL-CAPS headings are the other two shapes
    /// `is_numbered_section_heading` accepts; both must block a merge.
    #[test]
    fn should_not_merge_roman_or_allcaps_numbered_headings() {
        let mut paragraphs = vec![
            make_body_paragraph_at("III. Scope", 11.0, 700.0),
            make_body_paragraph_at("IV. Results", 11.0, 686.0),
            make_body_paragraph_at("5. CONCLUSIONS", 11.0, 672.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            3,
            "roman and ALL-CAPS numbered headings must not merge"
        );
        assert_eq!(first_line_text(&paragraphs[0]), "III. Scope");
        assert_eq!(first_line_text(&paragraphs[1]), "IV. Results");
        assert_eq!(first_line_text(&paragraphs[2]), "5. CONCLUSIONS");
    }

    /// A single-level number followed by mixed-case text is a LIST item, not a
    /// section heading, so the guard must not fire on it — this pins the
    /// conservative boundary of the predicate the fix relies on.
    #[test]
    fn should_still_merge_single_level_mixed_case_numbering() {
        let mut paragraphs = vec![
            make_body_paragraph_at("De procedure verloopt als volgt", 11.0, 700.0),
            make_body_paragraph_at("1. Eerste stap in het proces", 11.0, 686.0),
        ];
        merge_continuation_paragraphs(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            1,
            "'1. Eerste stap' is list-shaped, not a section heading; the guard must not fire"
        );
    }

    #[test]
    fn test_starts_with_lowercase_continuation_fn() {
        let para_lower = make_body_paragraph("and furthermore", 12.0);
        assert!(starts_with_lowercase_continuation(&para_lower));

        let para_upper = make_body_paragraph("Furthermore", 12.0);
        assert!(!starts_with_lowercase_continuation(&para_upper));
    }

    #[test]
    fn test_merge_clears_precomputed_text_on_heuristic_path() {
        let mut p1 = make_body_paragraph("een indicative", 12.0);
        p1.text = "een indicative".to_string();
        let mut p2 = make_body_paragraph("van toenemende merkbekendheid", 12.0);
        p2.text = "van toenemende merkbekendheid".to_string();
        let mut paragraphs = vec![p1, p2];
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1, "lowercase continuation should merge");
        assert!(
            paragraphs[0].text.is_empty(),
            "merged paragraph must clear pre-computed text so assembly joins from segments"
        );
        assert_eq!(paragraphs[0].lines.len(), 2, "both lines must be present after merge");
    }

    #[test]
    fn test_merge_struct_tree_path_text_stays_empty() {
        let mut paragraphs = vec![
            make_body_paragraph("first sentence without terminator", 12.0),
            make_body_paragraph("second continues here", 12.0),
        ];
        assert!(paragraphs[0].text.is_empty());
        merge_continuation_paragraphs(&mut paragraphs);
        assert_eq!(paragraphs.len(), 1);
        assert!(paragraphs[0].text.is_empty());
    }
}
