//! OCR-to-structure adapters: convert xberg internal types into the PDF
//! structure pipeline's paragraph representation.
// `types` is used by the OCR conversion helpers (`feature = "ocr"`) and by the
// unused when only `ocr-pipeline` is on without `layout-detection`, as in the
// WASM `ocr-wasm` feature set.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
use super::types;

/// Convert an OCR-produced [`crate::types::internal::InternalDocument`] into a vec of [`types::PdfParagraph`]s
/// for the structure assembly pipeline.
///
/// Coordinates are in image-space (y=0 at top) and are flipped to PDF-space
/// (y=0 at bottom) using `page_height_px`.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
#[allow(dead_code)]
pub(crate) fn ocr_doc_to_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height_px: u32,
) -> Vec<types::PdfParagraph> {
    use crate::types::internal::ElementKind;
    let page_h = page_height_px as f32;
    let mut result = Vec::new();
    let mut previous_block_id = None;

    for element in &doc.elements {
        if !matches!(element.kind, ElementKind::OcrText { .. }) || element.text.trim().is_empty() {
            previous_block_id = None;
            continue;
        }
        let block_id = hocr_block_id(element);
        let paragraph = make_ocr_block_paragraph(element, page_h);
        if block_id.is_some() && block_id == previous_block_id {
            if let Some(current) = result.last_mut() {
                merge_ocr_block_paragraph(current, paragraph);
            } else {
                result.push(paragraph);
            }
        } else {
            result.push(paragraph);
        }
        previous_block_id = block_id;
    }

    trace_conversion(doc, &result);
    result
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn hocr_block_id(element: &crate::types::internal::InternalElement) -> Option<&str> {
    const HOCR_BLOCK_ID_ATTRIBUTE: &str = "hocr_block_id";
    const MAX_HOCR_BLOCK_FRAGMENT_LINES: usize = 6;

    if element.text.lines().count() > MAX_HOCR_BLOCK_FRAGMENT_LINES {
        return None;
    }

    element
        .attributes
        .as_ref()?
        .get(HOCR_BLOCK_ID_ATTRIBUTE)
        .map(String::as_str)
        .filter(|block_id| !block_id.is_empty())
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn merge_ocr_block_paragraph(current: &mut types::PdfParagraph, next: types::PdfParagraph) {
    current.text.push('\n');
    current.text.push_str(&next.text);
    current.lines.extend(next.lines);
    current.block_bbox = match (current.block_bbox, next.block_bbox) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (bbox @ Some(_), None) | (None, bbox @ Some(_)) => bbox,
        (None, None) => None,
    };
    current.word_count = types::PdfParagraph::compute_word_count(&current.text, &current.lines);
}

/// Convert unstructured OCR text into page paragraphs without inventing geometry.
///
/// Line endings are normalized first: OCR page text is not guaranteed to be LF-only.
/// The VLM backend returns the model's markdown verbatim out of an HTTP JSON body
/// (`crate::llm::vlm_ocr`), which routinely carries `\r\n`, and nothing on the way
/// here rewrites it. Splitting raw would fold a whole page into one paragraph (#316).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
pub(crate) fn ocr_text_to_paragraphs(text: &str) -> Vec<types::PdfParagraph> {
    crate::extraction::transform::normalize_line_endings(text)
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| make_ocr_paragraph(paragraph.to_string(), Vec::new(), None))
        .collect()
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
pub(crate) fn ocr_doc_to_layout_paragraphs(
    doc: &crate::types::internal::InternalDocument,
    page_height_px: u32,
    hints: &[types::LayoutHint],
    min_confidence: f32,
    min_containment: f32,
) -> Vec<types::PdfParagraph> {
    use crate::types::internal::ElementKind;
    let page_height = page_height_px as f32;
    let mut all_lines = Vec::new();
    let mut all_hint_indices = Vec::new();
    let mut element_indices = Vec::new();
    let mut block_ids = Vec::new();
    let elements = doc
        .elements
        .iter()
        .filter(|element| matches!(element.kind, ElementKind::OcrText { .. }))
        .filter(|element| !element.text.trim().is_empty())
        .collect::<Vec<_>>();

    for (element_index, element) in elements.iter().enumerate() {
        let promote_logo_title = element_index == 0
            && should_promote_logo_followed_by_title(
                element,
                elements.get(1).copied(),
                page_height,
                hints,
                min_confidence,
            );
        let mut lines = make_ocr_line_paragraphs(element, page_height);
        let selected = super::layout_classify::apply_layout_overrides_with_matches(
            &mut lines,
            hints,
            min_confidence,
            min_containment,
            None,
        );
        let mut hint_indices = compatible_hint_indices(&lines, hints, selected, min_containment);
        if promote_logo_title {
            promote_second_line_to_title(&mut lines, &mut hint_indices);
        }
        element_indices.extend(std::iter::repeat_n(element_index, lines.len()));
        block_ids.extend(std::iter::repeat_n(
            hocr_block_id(element).map(str::to_owned),
            lines.len(),
        ));
        all_lines.extend(lines);
        all_hint_indices.extend(hint_indices);
    }

    let result = regroup_layout_lines_by_element(all_lines, all_hint_indices, element_indices, block_ids);
    trace_conversion(doc, &result);
    result
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
pub(crate) fn promote_anchored_ordered_list_sequences(pages: &mut [Vec<types::PdfParagraph>]) {
    const MAX_INTERVENING_PARAGRAPHS: usize = 8;

    let positions = nonempty_paragraph_positions(pages);
    let mut promotions = Vec::new();

    for (position_index, &(page_index, paragraph_index)) in positions.iter().enumerate() {
        let anchor = &pages[page_index][paragraph_index];
        let Some(anchor_value) = anchored_numeric_list_value(anchor) else {
            continue;
        };
        let Some(second_value) = anchor_value.checked_add(1) else {
            continue;
        };
        let Some(third_value) = anchor_value.checked_add(2) else {
            continue;
        };
        let Some(second_index) = find_ordered_list_successor(
            pages,
            &positions,
            position_index,
            second_value,
            MAX_INTERVENING_PARAGRAPHS,
        ) else {
            continue;
        };
        let Some(third_index) =
            find_ordered_list_successor(pages, &positions, second_index, third_value, MAX_INTERVENING_PARAGRAPHS)
        else {
            continue;
        };

        promotions.extend([positions[second_index], positions[third_index]]);
    }

    promotions.sort_unstable();
    promotions.dedup();
    for (page_index, paragraph_index) in promotions {
        let paragraph = &mut pages[page_index][paragraph_index];
        paragraph.is_list_item = true;
        paragraph.layout_class = Some(types::LayoutHintClass::ListItem);
    }
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn nonempty_paragraph_positions(pages: &[Vec<types::PdfParagraph>]) -> Vec<(usize, usize)> {
    pages
        .iter()
        .enumerate()
        .flat_map(|(page_index, paragraphs)| {
            paragraphs
                .iter()
                .enumerate()
                .filter(|(_, paragraph)| !paragraph.text.trim().is_empty())
                .map(move |(paragraph_index, _)| (page_index, paragraph_index))
        })
        .collect()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn anchored_numeric_list_value(paragraph: &types::PdfParagraph) -> Option<u16> {
    paragraph.is_list_item.then(|| numeric_list_value(paragraph)).flatten()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn numeric_list_value(paragraph: &types::PdfParagraph) -> Option<u16> {
    let marker = super::list_marker::parse_ordered_list_marker(&paragraph.text)?;
    (marker.has_content && marker.has_separator)
        .then_some(marker.numeric_value)
        .flatten()
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn find_ordered_list_successor(
    pages: &[Vec<types::PdfParagraph>],
    positions: &[(usize, usize)],
    predecessor_index: usize,
    expected_value: u16,
    max_intervening_paragraphs: usize,
) -> Option<usize> {
    let first_candidate = predecessor_index + 1;
    if first_candidate >= positions.len() {
        return None;
    }
    let last_candidate = (first_candidate + max_intervening_paragraphs).min(positions.len().saturating_sub(1));
    for (offset, &(page_index, paragraph_index)) in positions[first_candidate..=last_candidate].iter().enumerate() {
        let candidate_index = first_candidate + offset;
        let paragraph = &pages[page_index][paragraph_index];
        if let Some(actual_value) = numeric_list_value(paragraph) {
            return (actual_value == expected_value && is_ordered_list_candidate(paragraph)).then_some(candidate_index);
        }
    }
    None
}

#[cfg(all(any(feature = "ocr", feature = "ocr-pipeline"), feature = "layout-detection"))]
fn is_ordered_list_candidate(paragraph: &types::PdfParagraph) -> bool {
    paragraph.heading_level.is_none()
        && !paragraph.is_list_item
        && !paragraph.is_code_block
        && !paragraph.is_formula
        && !paragraph.is_page_furniture
        && !super::classify::is_numbered_section_heading(&paragraph.text)
        && matches!(
            paragraph.layout_class,
            None | Some(types::LayoutHintClass::Text | types::LayoutHintClass::Other)
        )
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn should_promote_logo_followed_by_title(
    element: &crate::types::internal::InternalElement,
    next_element: Option<&crate::types::internal::InternalElement>,
    page_height: f32,
    hints: &[types::LayoutHint],
    min_confidence: f32,
) -> bool {
    const MAX_LOGO_CHARACTERS: usize = 12;
    const MAX_LOGO_WORDS: usize = 2;
    const MIN_TITLE_WORDS: usize = 2;
    const MAX_TITLE_WORDS: usize = 12;
    const MAX_TITLE_CHARACTERS: usize = 120;

    let mut lines = element.text.lines();
    let Some(logo) = lines.next().map(str::trim) else {
        return false;
    };
    let Some(title) = lines.next().map(str::trim) else {
        return false;
    };
    if lines.next().is_some() || logo.is_empty() || title.is_empty() {
        return false;
    }

    !has_semantic_heading_hint(hints, min_confidence)
        && is_uppercase_logo(logo, MAX_LOGO_CHARACTERS, MAX_LOGO_WORDS)
        && is_conservative_title(title, MIN_TITLE_WORDS, MAX_TITLE_WORDS, MAX_TITLE_CHARACTERS)
        && next_element.is_some_and(|next| follows_with_prose(element, next, page_height))
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn has_semantic_heading_hint(hints: &[types::LayoutHint], min_confidence: f32) -> bool {
    hints.iter().any(|hint| {
        hint.confidence >= min_confidence
            && matches!(
                hint.class_name,
                types::LayoutHintClass::Title | types::LayoutHintClass::SectionHeader
            )
    })
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn is_uppercase_logo(text: &str, max_characters: usize, max_words: usize) -> bool {
    let alphabetic = text
        .chars()
        .filter(|character| character.is_alphabetic())
        .collect::<Vec<_>>();
    (2..=max_characters).contains(&text.chars().count())
        && text.split_whitespace().count() <= max_words
        && alphabetic.len() >= 2
        && alphabetic.iter().all(|character| character.is_uppercase())
        && text
            .chars()
            .all(|character| character.is_alphabetic() || character.is_whitespace())
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn is_conservative_title(text: &str, min_words: usize, max_words: usize, max_characters: usize) -> bool {
    let word_count = text.split_whitespace().count();
    let starts_uppercase = text
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(|character| character.is_uppercase());
    (min_words..=max_words).contains(&word_count)
        && text.chars().count() <= max_characters
        && starts_uppercase
        && text.chars().any(|character| character.is_lowercase())
        && !text.ends_with(['.', '!', '?', ':', ';', ','])
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn follows_with_prose(
    element: &crate::types::internal::InternalElement,
    next_element: &crate::types::internal::InternalElement,
    page_height: f32,
) -> bool {
    const MIN_PROSE_WORDS: usize = 8;
    const MIN_FIRST_BLOCK_TOP_FRACTION: f32 = 0.65;
    const MAX_FIRST_BLOCK_HEIGHT_FRACTION: f32 = 0.15;
    const MAX_PROSE_GAP_FRACTION: f32 = 0.15;

    let prose = next_element.text.trim();
    let Some((_, first_bottom, _, first_top)) = pdf_block_bbox(element, page_height) else {
        return false;
    };
    let Some((_, _, _, prose_top)) = pdf_block_bbox(next_element, page_height) else {
        return false;
    };
    let prose_gap = first_bottom - prose_top;
    first_top >= page_height * MIN_FIRST_BLOCK_TOP_FRACTION
        && first_top - first_bottom <= page_height * MAX_FIRST_BLOCK_HEIGHT_FRACTION
        && (0.0..=page_height * MAX_PROSE_GAP_FRACTION).contains(&prose_gap)
        && prose.split_whitespace().count() >= MIN_PROSE_WORDS
        && prose.chars().any(|character| character.is_lowercase())
        && prose.ends_with(['.', '!', '?'])
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn promote_second_line_to_title(lines: &mut [types::PdfParagraph], hint_indices: &mut [Option<usize>]) {
    const TITLE_LINE_INDEX: usize = 1;
    let Some(title) = lines.get_mut(TITLE_LINE_INDEX) else {
        return;
    };
    if has_structural_override(title) {
        return;
    }
    title.heading_level = Some(1);
    title.layout_class = Some(types::LayoutHintClass::Title);
    if let Some(hint_index) = hint_indices.get_mut(TITLE_LINE_INDEX) {
        *hint_index = None;
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn trace_conversion(doc: &crate::types::internal::InternalDocument, result: &[types::PdfParagraph]) {
    tracing::debug!(
        input_elements = doc
            .elements
            .iter()
            .filter(|element| matches!(element.kind, crate::types::internal::ElementKind::OcrText { .. }))
            .count(),
        output_paragraphs = result.len(),
        total_text_chars = result.iter().map(|paragraph| paragraph.text.len()).sum::<usize>(),
        "ocr_doc_to_paragraphs"
    );
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_block_paragraph(
    element: &crate::types::internal::InternalElement,
    page_height: f32,
) -> types::PdfParagraph {
    let block_bbox = pdf_block_bbox(element, page_height);
    let line_paragraphs = make_ocr_line_paragraphs(element, page_height);
    let lines = line_paragraphs
        .iter()
        .flat_map(|paragraph| paragraph.lines.iter().cloned())
        .collect();
    make_ocr_paragraph(element.text.clone(), lines, block_bbox)
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_line_paragraphs(
    element: &crate::types::internal::InternalElement,
    page_height: f32,
) -> Vec<types::PdfParagraph> {
    let block_bbox = pdf_block_bbox(element, page_height);
    let text_lines = element.text.split('\n').collect::<Vec<_>>();
    let line_count = text_lines.len().max(1);

    text_lines
        .into_iter()
        .enumerate()
        .map(|(line_index, text)| make_ocr_line_paragraph(text, line_index, line_count, block_bbox))
        .collect()
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn pdf_block_bbox(element: &crate::types::internal::InternalElement, page_height: f32) -> Option<(f32, f32, f32, f32)> {
    element.bbox.as_ref().map(|bbox| {
        (
            bbox.x0 as f32,
            page_height - bbox.y1 as f32,
            bbox.x1 as f32,
            page_height - bbox.y0 as f32,
        )
    })
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_line_paragraph(
    text: &str,
    line_index: usize,
    line_count: usize,
    block_bbox: Option<(f32, f32, f32, f32)>,
) -> types::PdfParagraph {
    const DEFAULT_FONT_SIZE: f32 = 12.0;
    const DEFAULT_LINE_WIDTH: f32 = 100.0;

    let line_height = block_bbox
        .map(|(_, bottom, _, top)| (top - bottom) / line_count as f32)
        .unwrap_or(DEFAULT_FONT_SIZE);
    let line_bbox = block_bbox.map(|(left, _bottom, right, top)| {
        let line_top = top - line_index as f32 * line_height;
        (left, line_top - line_height, right, line_top)
    });
    let (x, baseline_y, width) = line_bbox
        .map(|(left, bottom, right, _)| (left, bottom, right - left))
        .unwrap_or((0.0, 0.0, DEFAULT_LINE_WIDTH));
    let lines = if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![make_ocr_pdf_line(
            text,
            x,
            baseline_y,
            width,
            line_height,
            DEFAULT_FONT_SIZE,
        )]
    };
    make_ocr_paragraph(text.to_string(), lines, line_bbox)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn compatible_hint_indices(
    lines: &[types::PdfParagraph],
    hints: &[types::LayoutHint],
    selected: Vec<Option<usize>>,
    min_containment: f32,
) -> Vec<Option<usize>> {
    let mut compatible = vec![None; lines.len()];
    let mut previous_list_hint = None;
    for (index, line) in lines.iter().enumerate() {
        let actual = selected[index].filter(|&hint_index| {
            hints
                .get(hint_index)
                .is_some_and(|hint| classification_matches_hint(line, hint.class_name))
        });
        compatible[index] = actual
            .or_else(|| inherit_list_continuation(line, selected[index], previous_list_hint, hints, min_containment));
        previous_list_hint = compatible[index].filter(|&hint_index| {
            hints[hint_index].class_name == types::LayoutHintClass::ListItem && !line.text.trim().is_empty()
        });
    }
    compatible
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn classification_matches_hint(paragraph: &types::PdfParagraph, class_name: types::LayoutHintClass) -> bool {
    use types::LayoutHintClass as L;
    if paragraph.layout_class != Some(class_name) {
        return false;
    }
    match class_name {
        L::Title | L::SectionHeader => paragraph.heading_level.is_some(),
        L::Code => paragraph.is_code_block,
        L::Formula => paragraph.is_formula,
        L::ListItem => paragraph.is_list_item,
        L::Text => true,
        L::Caption | L::Footnote => true,
        L::PageHeader | L::PageFooter | L::Picture => paragraph.is_page_furniture,
        _ => false,
    }
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn inherit_list_continuation(
    paragraph: &types::PdfParagraph,
    selected_hint: Option<usize>,
    previous_list_hint: Option<usize>,
    hints: &[types::LayoutHint],
    min_containment: f32,
) -> Option<usize> {
    let hint_index = previous_list_hint?;
    let hint = hints.get(hint_index)?;
    (selected_hint.is_none()
        && !paragraph.text.trim().is_empty()
        && hint_containment(paragraph.block_bbox?, hint) >= min_containment)
        .then_some(hint_index)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn hint_containment(bbox: (f32, f32, f32, f32), hint: &types::LayoutHint) -> f32 {
    let intersection_width = (bbox.2.min(hint.right) - bbox.0.max(hint.left)).max(0.0);
    let intersection_height = (bbox.3.min(hint.top) - bbox.1.max(hint.bottom)).max(0.0);
    let paragraph_area = (bbox.2 - bbox.0).max(0.0) * (bbox.3 - bbox.1).max(0.0);
    if paragraph_area > 0.0 {
        intersection_width * intersection_height / paragraph_area
    } else {
        0.0
    }
}

#[cfg(all(test, feature = "ocr", feature = "layout-detection"))]
fn regroup_layout_lines(lines: Vec<types::PdfParagraph>, hint_indices: Vec<Option<usize>>) -> Vec<types::PdfParagraph> {
    let element_indices = vec![0; lines.len()];
    let block_ids = vec![None; lines.len()];
    regroup_layout_lines_by_element(lines, hint_indices, element_indices, block_ids)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn regroup_layout_lines_by_element(
    lines: Vec<types::PdfParagraph>,
    hint_indices: Vec<Option<usize>>,
    element_indices: Vec<usize>,
    block_ids: Vec<Option<String>>,
) -> Vec<types::PdfParagraph> {
    let mut result = Vec::new();
    let mut body_lines = Vec::new();
    let mut body_region = None;
    let mut groups = group_by_hint(lines, hint_indices, element_indices, block_ids);

    for group in groups.drain(..) {
        if group.lines.iter().any(has_structural_override) {
            push_body_group(&mut result, std::mem::take(&mut body_lines));
            body_region = None;
            if let Some(paragraph) = merge_structural_group(group.lines) {
                result.push(paragraph);
            }
        } else {
            let lines_are_near = body_lines
                .last()
                .zip(group.lines.first())
                .is_some_and(|(previous, current)| layout_lines_are_near(previous, current));
            let same_region = lines_are_near
                && body_region
                    .as_ref()
                    .is_some_and(|(hint_index, element_index, block_id)| {
                        (group.hint_index.is_some() && *hint_index == group.hint_index)
                            || *element_index == group.element_index
                            || (group.block_id.is_some() && *block_id == group.block_id)
                    });
            if !body_lines.is_empty() && !same_region {
                push_body_group(&mut result, std::mem::take(&mut body_lines));
            }
            if body_lines.is_empty() {
                body_region = Some((group.hint_index, group.element_index, group.block_id));
            }
            body_lines.extend(group.lines);
        }
    }
    push_body_group(&mut result, body_lines);
    result
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
struct LayoutLineGroup {
    hint_index: Option<usize>,
    element_index: usize,
    block_id: Option<String>,
    lines: Vec<types::PdfParagraph>,
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn group_by_hint(
    lines: Vec<types::PdfParagraph>,
    hint_indices: Vec<Option<usize>>,
    element_indices: Vec<usize>,
    block_ids: Vec<Option<String>>,
) -> Vec<LayoutLineGroup> {
    let mut groups: Vec<LayoutLineGroup> = Vec::new();
    for (((line, hint_index), element_index), block_id) in
        lines.into_iter().zip(hint_indices).zip(element_indices).zip(block_ids)
    {
        if let Some(group) = groups.last_mut()
            && hint_index.is_some()
            && group.hint_index == hint_index
            && group
                .lines
                .last()
                .is_some_and(|previous| layout_lines_are_near(previous, &line))
        {
            group.lines.push(line);
        } else {
            groups.push(LayoutLineGroup {
                hint_index,
                element_index,
                block_id,
                lines: vec![line],
            });
        }
    }
    groups
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn layout_lines_are_near(previous: &types::PdfParagraph, current: &types::PdfParagraph) -> bool {
    const MAX_GAP_IN_LINE_HEIGHTS: f32 = 1.5;

    let (Some(previous_bbox), Some(current_bbox)) = (previous.block_bbox, current.block_bbox) else {
        return false;
    };
    let previous_height = (previous_bbox.3 - previous_bbox.1).abs();
    let current_height = (current_bbox.3 - current_bbox.1).abs();
    let line_height = previous_height.max(current_height).max(1.0);
    let vertical_gap = if current_bbox.3 < previous_bbox.1 {
        previous_bbox.1 - current_bbox.3
    } else if previous_bbox.3 < current_bbox.1 {
        current_bbox.1 - previous_bbox.3
    } else {
        0.0
    };
    vertical_gap <= line_height * MAX_GAP_IN_LINE_HEIGHTS
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn has_structural_override(paragraph: &types::PdfParagraph) -> bool {
    !paragraph.text.trim().is_empty()
        && (paragraph.heading_level.is_some()
            || paragraph.is_list_item
            || paragraph.is_code_block
            || paragraph.is_formula
            || paragraph.is_page_furniture
            || matches!(
                paragraph.layout_class,
                Some(types::LayoutHintClass::Caption | types::LayoutHintClass::Footnote)
            ))
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn push_body_group(result: &mut Vec<types::PdfParagraph>, lines: Vec<types::PdfParagraph>) {
    let lines = trim_blank_boundaries(lines);
    if lines.is_empty() {
        return;
    }
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return;
    }
    let bbox = union_bboxes(&lines);
    let layout_class = lines.iter().find_map(|line| line.layout_class);
    let pdf_lines = lines.into_iter().flat_map(|line| line.lines).collect();
    let mut paragraph = make_ocr_paragraph(text, pdf_lines, bbox);
    paragraph.layout_class = layout_class;
    result.push(paragraph);
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn merge_structural_group(lines: Vec<types::PdfParagraph>) -> Option<types::PdfParagraph> {
    let lines = trim_blank_boundaries(lines);
    let template = lines.iter().find(|line| has_structural_override(line))?.clone();
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let bbox = union_bboxes(&lines);
    let pdf_lines = lines.into_iter().flat_map(|line| line.lines).collect::<Vec<_>>();
    let mut merged = template;
    merged.word_count = types::PdfParagraph::compute_word_count(&text, &pdf_lines);
    merged.text = text;
    merged.lines = pdf_lines;
    merged.block_bbox = bbox;
    Some(merged)
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn trim_blank_boundaries(mut lines: Vec<types::PdfParagraph>) -> Vec<types::PdfParagraph> {
    let first_content = lines
        .iter()
        .position(|line| !line.text.trim().is_empty())
        .unwrap_or(lines.len());
    let retained = lines[first_content..]
        .iter()
        .rposition(|line| !line.text.trim().is_empty())
        .map_or(0, |index| index + 1);
    lines.drain(..first_content);
    lines.truncate(retained);
    lines
}

#[cfg(all(feature = "ocr", feature = "layout-detection"))]
fn union_bboxes(lines: &[types::PdfParagraph]) -> Option<(f32, f32, f32, f32)> {
    lines
        .iter()
        .filter_map(|line| line.block_bbox)
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_paragraph(
    text: String,
    lines: Vec<types::PdfLine>,
    block_bbox: Option<(f32, f32, f32, f32)>,
) -> types::PdfParagraph {
    const DEFAULT_FONT_SIZE: f32 = 12.0;
    types::PdfParagraph {
        word_count: types::PdfParagraph::compute_word_count(&text, &lines),
        text,
        lines,
        dominant_font_size: DEFAULT_FONT_SIZE,
        heading_level: None,
        is_bold: false,
        is_list_item: false,
        is_code_block: false,
        is_formula: false,
        is_page_furniture: false,
        layout_class: None,
        layout_region_path: None,
        caption_for: None,
        block_bbox,
    }
}

#[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
fn make_ocr_pdf_line(
    text: &str,
    x: f32,
    baseline_y: f32,
    width: f32,
    line_height: f32,
    font_size: f32,
) -> types::PdfLine {
    let segment = crate::pdf::hierarchy::SegmentData {
        text: text.to_string(),
        x,
        y: baseline_y,
        width,
        height: line_height,
        font_size,
        is_bold: false,
        is_italic: false,
        is_monospace: false,
        baseline_y,
        assigned_role: None,
    };
    types::PdfLine {
        segments: vec![segment],
        baseline_y,
        dominant_font_size: font_size,
        is_bold: false,
        is_monospace: false,
    }
}

#[cfg(all(feature = "ocr", test))]
mod tests {
    use super::*;
    use crate::types::extraction::BoundingBox;
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
    use crate::types::ocr_elements::OcrElementLevel;

    #[test]
    fn test_ocr_doc_soft_wrapped_body_stays_one_paragraph() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "First soft-wrapped body line\ncontinues on the next visual line",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 50.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(
            paragraphs[0].text,
            "First soft-wrapped body line\ncontinues on the next visual line"
        );
        assert_eq!(paragraphs[0].lines.len(), 2);
    }

    #[test]
    fn test_ocr_doc_merges_only_consecutive_paragraphs_in_same_hocr_block() {
        let mut same_block = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        set_hocr_block_ids(&mut same_block, &[Some("block_1_1"), Some("block_1_1")]);
        let merged = ocr_doc_to_paragraphs(&same_block, 1000);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "First\nSecond");

        let mut different_blocks = same_block.clone();
        set_hocr_block_ids(&mut different_blocks, &[Some("block_1_1"), Some("block_1_2")]);
        assert_eq!(ocr_doc_to_paragraphs(&different_blocks, 1000).len(), 2);

        let no_blocks = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        assert_eq!(ocr_doc_to_paragraphs(&no_blocks, 1000).len(), 2);

        let mut long_paragraphs = layout_line_document(&[
            ("One\nTwo\nThree\nFour\nFive\nSix\nSeven", 100.0, 100.0, 500.0, 240.0),
            (
                "Eight\nNine\nTen\nEleven\nTwelve\nThirteen\nFourteen",
                100.0,
                240.0,
                500.0,
                380.0,
            ),
        ]);
        set_hocr_block_ids(&mut long_paragraphs, &[Some("block_1_1"), Some("block_1_1")]);
        assert_eq!(ocr_doc_to_paragraphs(&long_paragraphs, 1000).len(), 2);
    }

    /// Test that OCR elements with mixed content and blank lines preserve all text.
    #[test]
    fn test_ocr_doc_preserves_mixed_content_with_blanks() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "line1\n\nline3",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "line1\n\nline3");
        assert_eq!(paragraphs[0].word_count, 2);
        assert_eq!(paragraphs[0].lines.len(), 2);
    }

    /// Test that whitespace-only OCR elements are filtered out (correct behavior).
    #[test]
    fn test_ocr_doc_filters_whitespace_only_elements() {
        let mut doc = InternalDocument::new("test");
        let mut elem1 = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "   \n  \n  ",
            0,
        );
        elem1.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem1);

        let mut elem2 = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "real content",
            0,
        );
        elem2.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 80.0,
            x1: 100.0,
            y1: 140.0,
        });
        doc.push_element(elem2);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);

        assert_eq!(paragraphs.len(), 1, "Should filter out whitespace-only element");
        assert_eq!(paragraphs[0].text, "real content");
    }

    /// Test that whitespace-only lines and their exact text remain in the OCR block.
    #[test]
    fn test_ocr_doc_whitespace_lines_text_preserved() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Para1\n   \nPara2",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Para1\n   \nPara2");
        assert_eq!(paragraphs[0].lines.len(), 2);
        let line_height = (70.0 - 10.0) / 3.0;
        let para_1_y = 1000.0 - 10.0 - line_height;
        let para_2_y = 1000.0 - 10.0 - 3.0 * line_height;
        assert!(
            (paragraphs[0].lines[0].baseline_y - para_1_y).abs() < 0.1,
            "Line 1 Y position incorrect"
        );
        assert!(
            (paragraphs[0].lines[1].baseline_y - para_2_y).abs() < 0.1,
            "Line 2 Y position incorrect"
        );
    }

    /// Test that blank lines in OCR elements don't affect vertical positioning.
    /// When text contains blank lines (e.g., "A\n\nC"), the lines array should still
    /// have correct y-positions (0 for A, 2*line_height for C, not 1*line_height).
    /// This ensures correct sorting order when multiple paragraphs are interleaved.
    #[test]
    fn test_ocr_doc_blank_lines_preserve_vertical_spacing() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "Line1\n\nLine3",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 90.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Line1\n\nLine3");
        assert_eq!(paragraphs[0].lines.len(), 2);

        let expected_line_height = 80.0 / 3.0;

        assert!(
            (paragraphs[0].lines[0].baseline_y - (990.0 - expected_line_height)).abs() < 0.1,
            "Line1 baseline is incorrect: {}",
            paragraphs[0].lines[0].baseline_y
        );
        let first_segment = &paragraphs[0].lines[0].segments[0];
        assert!((first_segment.y + first_segment.height - 990.0).abs() < 0.1);
        assert!(
            (paragraphs[0].lines[1].baseline_y - (990.0 - 3.0 * expected_line_height)).abs() < 0.1,
            "Line3 baseline is incorrect: {}",
            paragraphs[0].lines[1].baseline_y
        );
    }

    /// Test that OCR elements with content followed by blanks preserve content.
    #[test]
    fn test_ocr_doc_preserves_content_before_blanks() {
        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Line,
            },
            "important\n\n",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 100.0,
            y1: 70.0,
        });
        doc.push_element(elem);

        let paragraphs = ocr_doc_to_paragraphs(&doc, 1000);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "important\n\n");
        assert_eq!(paragraphs[0].word_count, 1);
        assert_eq!(
            paragraphs[0].lines.len(),
            1,
            "Only the non-blank line should be in lines array"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_ocr_block_applies_line_sized_layout_hint_only_to_matching_line() {
        use crate::pdf::structure::types::{LayoutHint, LayoutHintClass};

        let mut doc = InternalDocument::new("test");
        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "Document title\nFirst body line\nSecond body line",
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 160.0,
        });
        doc.push_element(elem);

        let hints = [LayoutHint {
            class_name: LayoutHintClass::Title,
            confidence: 0.95,
            left: 100.0,
            bottom: 880.0,
            right: 500.0,
            top: 900.0,
        }];
        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[1].text, "First body line\nSecond body line");
        assert_eq!(paragraphs[0].block_bbox, Some((100.0, 880.0, 500.0, 900.0)));
        assert_eq!(paragraphs[1].block_bbox, Some((100.0, 840.0, 500.0, 880.0)));
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].heading_level, None);

        let assembled = crate::pdf::structure::assemble_internal_document(vec![paragraphs], &[], None, &[]);
        assert_eq!(assembled.elements.len(), 2);
        assert_eq!(assembled.elements[0].kind, ElementKind::Heading { level: 1 });
        assert_eq!(assembled.elements[1].kind, ElementKind::Paragraph);
        assert_eq!(assembled.elements[1].text, "First body line\nSecond body line");
    }

    #[cfg(feature = "layout-detection")]
    fn layout_test_document(text: &str, line_count: u32) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            text,
            0,
        );
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 100.0 + f64::from(line_count * 20),
        });
        doc.push_element(element);
        doc
    }

    #[cfg(feature = "layout-detection")]
    fn layout_test_hint(class_name: types::LayoutHintClass, bottom: f32, top: f32) -> types::LayoutHint {
        types::LayoutHint {
            class_name,
            confidence: 0.95,
            left: 100.0,
            bottom,
            right: 500.0,
            top,
        }
    }

    fn layout_line_document(lines: &[(&str, f64, f64, f64, f64)]) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        for &(text, x0, y0, x1, y1) in lines {
            let mut element = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Line,
                },
                text,
                0,
            );
            element.bbox = Some(BoundingBox { x0, y0, x1, y1 });
            doc.push_element(element);
        }
        doc
    }

    fn set_hocr_block_ids(doc: &mut InternalDocument, block_ids: &[Option<&str>]) {
        for (element, block_id) in doc.elements.iter_mut().zip(block_ids) {
            element.attributes = block_id.map(|block_id| {
                [("hocr_block_id".to_string(), block_id.to_string())]
                    .into_iter()
                    .collect()
            });
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_layout_merges_only_adjacent_lines_in_same_hocr_block() {
        let mut same_block = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        set_hocr_block_ids(&mut same_block, &[Some("block_1_1"), Some("block_1_1")]);
        let merged = ocr_doc_to_layout_paragraphs(&same_block, 1000, &[], 0.5, 0.2);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "First\nSecond");

        let mut different_blocks = same_block.clone();
        set_hocr_block_ids(&mut different_blocks, &[Some("block_1_1"), Some("block_1_2")]);
        assert_eq!(
            ocr_doc_to_layout_paragraphs(&different_blocks, 1000, &[], 0.5, 0.2).len(),
            2
        );

        let no_blocks = layout_line_document(&[
            ("First", 100.0, 100.0, 500.0, 120.0),
            ("Second", 100.0, 120.0, 500.0, 140.0),
        ]);
        assert_eq!(ocr_doc_to_layout_paragraphs(&no_blocks, 1000, &[], 0.5, 0.2).len(), 2);

        let mut long_paragraphs = layout_line_document(&[
            ("One\nTwo\nThree\nFour\nFive\nSix\nSeven", 100.0, 100.0, 500.0, 240.0),
            (
                "Eight\nNine\nTen\nEleven\nTwelve\nThirteen\nFourteen",
                100.0,
                240.0,
                500.0,
                380.0,
            ),
        ]);
        set_hocr_block_ids(&mut long_paragraphs, &[Some("block_1_1"), Some("block_1_1")]);
        assert_eq!(
            ocr_doc_to_layout_paragraphs(&long_paragraphs, 1000, &[], 0.5, 0.2).len(),
            2
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_coalesce_adjacent_ocr_elements_in_same_text_region() {
        let doc = layout_line_document(&[
            ("First wrapped line", 100.0, 100.0, 500.0, 120.0),
            ("continues in the same paragraph", 100.0, 120.0, 500.0, 140.0),
        ]);
        let hints = [layout_test_hint(types::LayoutHintClass::Text, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(
            paragraphs[0].text,
            "First wrapped line\ncontinues in the same paragraph"
        );
        assert_eq!(paragraphs[0].layout_class, Some(types::LayoutHintClass::Text));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_flush_body_regions_when_heading_separates_ocr_elements() {
        let doc = layout_line_document(&[
            ("Body before", 100.0, 100.0, 500.0, 120.0),
            ("Section title", 100.0, 120.0, 500.0, 140.0),
            ("Body after", 100.0, 140.0, 500.0, 160.0),
        ]);
        let hints = [
            layout_test_hint(types::LayoutHintClass::Text, 840.0, 900.0),
            layout_test_hint(types::LayoutHintClass::SectionHeader, 860.0, 880.0),
        ];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 3);
        assert_eq!(paragraphs[0].text, "Body before");
        assert_eq!(paragraphs[1].text, "Section title");
        assert_eq!(paragraphs[1].heading_level, Some(2));
        assert_eq!(paragraphs[2].text, "Body after");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_keep_adjacent_ocr_elements_in_distinct_text_regions_separate() {
        let doc = layout_line_document(&[
            ("Left column", 100.0, 100.0, 280.0, 120.0),
            ("Right column", 320.0, 100.0, 500.0, 120.0),
        ]);
        let hints = [
            types::LayoutHint {
                class_name: types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 100.0,
                bottom: 880.0,
                right: 280.0,
                top: 900.0,
            },
            types::LayoutHint {
                class_name: types::LayoutHintClass::Text,
                confidence: 0.95,
                left: 320.0,
                bottom: 880.0,
                right: 500.0,
                top: 900.0,
            },
        ];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Left column");
        assert_eq!(paragraphs[1].text, "Right column");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_keep_distant_ocr_elements_in_same_text_region_separate() {
        let doc = layout_line_document(&[
            ("First paragraph", 100.0, 100.0, 500.0, 120.0),
            ("Second paragraph", 100.0, 300.0, 500.0, 320.0),
        ]);
        let hints = [layout_test_hint(types::LayoutHintClass::Text, 680.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "First paragraph");
        assert_eq!(paragraphs[1].text, "Second paragraph");
    }

    #[cfg(feature = "layout-detection")]
    fn logo_title_test_document(first_block: &str, prose: Option<&str>) -> InternalDocument {
        let mut doc = InternalDocument::new("test");
        let mut first = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            first_block,
            0,
        );
        first.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 100.0,
            x1: 500.0,
            y1: 180.0,
        });
        doc.push_element(first);

        if let Some(prose) = prose {
            let mut body = InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                prose,
                0,
            );
            body.bbox = Some(BoundingBox {
                x0: 50.0,
                y0: 220.0,
                x1: 550.0,
                y1: 400.0,
            });
            doc.push_element(body);
        }
        doc
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_first_logo_block_promotes_following_title_without_reordering() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::Text,
            confidence: 0.97,
            left: 50.0,
            bottom: 750.0,
            right: 550.0,
            top: 920.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 3);
        assert_eq!(paragraphs[0].text, "IDRH");
        assert_eq!(paragraphs[0].heading_level, None);
        assert_eq!(paragraphs[1].text, "Non-text-searchable PDF");
        assert_eq!(paragraphs[1].heading_level, Some(1));
        assert_eq!(paragraphs[1].layout_class, Some(types::LayoutHintClass::Title));
        assert_eq!(paragraphs[2].text, prose);
        assert_eq!(paragraphs[2].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_requires_following_prose() {
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", None);

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &[], 0.5, 0.2);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "IDRH\nNon-text-searchable PDF");
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_preserves_non_logo_first_blocks() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        for first_block in [
            "Quarterly results\nRevenue increased",
            "Geschäftsstelle Ludwigstraße 23\n80539 München",
            "NIVE <¥ Rs), ,\nYr A %",
            "IDRH\nNon-text-searchable PDF.",
        ] {
            let doc = logo_title_test_document(first_block, Some(prose));

            let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &[], 0.5, 0.2);

            assert_eq!(paragraphs[0].text, first_block);
            assert_eq!(paragraphs[0].heading_level, None);
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_defers_to_semantic_heading_hint() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nNon-text-searchable PDF", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::SectionHeader,
            confidence: 0.95,
            left: 700.0,
            bottom: 700.0,
            right: 900.0,
            top: 750.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs[0].text, "IDRH\nNon-text-searchable PDF");
        assert_eq!(paragraphs[0].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_logo_title_fallback_preserves_existing_structural_override() {
        let prose = "This is a complete body sentence with enough words to identify ordinary prose.";
        let doc = logo_title_test_document("IDRH\nfunction title() {", Some(prose));
        let hints = [types::LayoutHint {
            class_name: types::LayoutHintClass::Code,
            confidence: 0.95,
            left: 100.0,
            bottom: 820.0,
            right: 500.0,
            top: 860.0,
        }];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs[0].text, "IDRH");
        assert_eq!(paragraphs[0].heading_level, None);
        assert!(paragraphs[1].is_code_block);
        assert_eq!(paragraphs[1].heading_level, None);
        assert_eq!(paragraphs[1].layout_class, Some(types::LayoutHintClass::Code));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_title_lines_under_same_hint_merge() {
        let doc = layout_test_document("A long document\nsubtitle line\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "A long document\nsubtitle line");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[0].block_bbox, Some((100.0, 860.0, 500.0, 900.0)));
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_multiline_code_lines_under_same_hint_merge() {
        let doc = layout_test_document("fn main() {\nprintln!(\"hello\");\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Code, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "fn main() {\nprintln!(\"hello\");");
        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejected_title_prose_does_not_merge_with_title() {
        let prose = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen \
                     sixteen seventeen eighteen nineteen twenty twentyone twentytwo twentythree twentyfour twentyfive";
        let doc = layout_test_document(&format!("Document title\n{prose}"), 2);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].text, prose);
        assert_eq!(paragraphs[1].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejected_code_prose_does_not_merge_with_code() {
        let prose = "This ordinary prose sentence contains many words, and it clearly should remain body text rather than code.";
        let doc = layout_test_document(&format!("fn main() {{\n{prose}"), 2);
        let hints = [layout_test_hint(types::LayoutHintClass::Code, 860.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "fn main() {");
        assert!(paragraphs[0].is_code_block);
        assert_eq!(paragraphs[1].text, prose);
        assert!(!paragraphs[1].is_code_block);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_wrapped_list_item_lines_under_same_hint_merge() {
        let doc = layout_test_document(
            "1. Wrapped item starts\nand continues here\nacross another line\nBody text",
            4,
        );
        let hints = [layout_test_hint(types::LayoutHintClass::ListItem, 840.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(
            paragraphs[0].text,
            "1. Wrapped item starts\nand continues here\nacross another line"
        );
        assert!(paragraphs[0].is_list_item);
        assert_eq!(paragraphs[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_title_split_trims_boundary_blank_before_assembled_body() {
        let doc = layout_test_document("Document title\n\nBody text", 3);
        let hints = [layout_test_hint(types::LayoutHintClass::Title, 880.0, 900.0)];
        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Document title");
        assert_eq!(paragraphs[1].text, "Body text");

        let assembled = crate::pdf::structure::assemble_internal_document(vec![paragraphs], &[], None, &[]);
        assert_eq!(assembled.elements.len(), 2);
        assert_eq!(assembled.elements[0].kind, ElementKind::Heading { level: 1 });
        assert_eq!(assembled.elements[1].kind, ElementKind::Paragraph);
        assert_eq!(assembled.elements[1].text, "Body text");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_unassociated_structural_line_does_not_capture_adjacent_body_lines() {
        let doc = layout_test_document("Promoted line\nBody one\nBody two", 3);
        let mut lines = make_ocr_line_paragraphs(&doc.elements[0], 1000.0);
        lines[0].heading_level = Some(1);

        let paragraphs = regroup_layout_lines(lines, vec![None, None, None]);

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].text, "Promoted line");
        assert_eq!(paragraphs[0].heading_level, Some(1));
        assert_eq!(paragraphs[1].text, "Body one\nBody two");
        assert_eq!(paragraphs[1].heading_level, None);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_picture_uses_canonical_match_identity() {
        let doc = layout_test_document("Detected picture region", 1);
        let hints = [layout_test_hint(types::LayoutHintClass::Picture, 880.0, 900.0)];

        let paragraphs = ocr_doc_to_layout_paragraphs(&doc, 1000, &hints, 0.5, 0.2);

        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].layout_class, Some(types::LayoutHintClass::Picture));
        assert!(paragraphs[0].is_page_furniture);
    }

    #[cfg(feature = "layout-detection")]
    fn ordered_list_test_paragraph(text: &str) -> types::PdfParagraph {
        make_ocr_paragraph(text.to_string(), Vec::new(), None)
    }

    #[cfg(feature = "layout-detection")]
    fn anchored_ordered_list_test_pages() -> Vec<Vec<types::PdfParagraph>> {
        let mut anchor = ordered_list_test_paragraph("1. First item");
        anchor.is_list_item = true;
        anchor.layout_class = Some(types::LayoutHintClass::ListItem);
        vec![
            vec![anchor, ordered_list_test_paragraph("First continuation")],
            vec![
                ordered_list_test_paragraph("2. Second item"),
                ordered_list_test_paragraph("Second continuation one"),
                ordered_list_test_paragraph("Second continuation two"),
                ordered_list_test_paragraph("Second continuation three"),
                ordered_list_test_paragraph("Second continuation four"),
                ordered_list_test_paragraph("Second continuation five"),
                ordered_list_test_paragraph("Second continuation six"),
                ordered_list_test_paragraph("Second continuation seven"),
                ordered_list_test_paragraph("3. Third item"),
            ],
        ]
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_promotes_complete_anchored_ordered_list_sequence_across_pages() {
        let mut pages = anchored_ordered_list_test_pages();
        pages[1].insert(8, ordered_list_test_paragraph("Second continuation eight"));
        let original_text = pages
            .iter()
            .flatten()
            .map(|paragraph| paragraph.text.clone())
            .collect::<Vec<_>>();

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(pages[1][0].is_list_item);
        assert_eq!(pages[1][0].layout_class, Some(types::LayoutHintClass::ListItem));
        assert!(pages[1][9].is_list_item);
        assert_eq!(pages[1][9].layout_class, Some(types::LayoutHintClass::ListItem));
        assert_eq!(
            pages
                .iter()
                .flatten()
                .map(|paragraph| paragraph.text.clone())
                .collect::<Vec<_>>(),
            original_text,
            "promotion must preserve intervening paragraphs and reading order"
        );
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_does_not_promote_unanchored_or_incomplete_ordered_sequences() {
        let mut unanchored = anchored_ordered_list_test_pages();
        unanchored[0][0].is_list_item = false;
        unanchored[0][0].layout_class = None;
        promote_anchored_ordered_list_sequences(&mut unanchored);
        assert!(!unanchored[1][0].is_list_item);
        assert!(!unanchored[1][8].is_list_item);

        let mut incomplete = anchored_ordered_list_test_pages();
        incomplete[1].pop();
        promote_anchored_ordered_list_sequences(&mut incomplete);
        assert!(!incomplete[1][0].is_list_item, "a two-item prefix must not mutate");
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_does_not_promote_broken_or_over_gap_ordered_sequences() {
        let mut broken = anchored_ordered_list_test_pages();
        broken[1][0].text = "3. Out of sequence".to_string();
        broken[1][8].text = "4. Still out of sequence".to_string();
        promote_anchored_ordered_list_sequences(&mut broken);
        assert!(!broken[1][0].is_list_item);
        assert!(!broken[1][8].is_list_item);

        let mut over_gap = anchored_ordered_list_test_pages();
        const EXTRA_CONTINUATIONS: usize = 8;
        for _ in 0..EXTRA_CONTINUATIONS {
            over_gap[0].insert(1, ordered_list_test_paragraph("Extra continuation"));
        }
        promote_anchored_ordered_list_sequences(&mut over_gap);
        assert!(!over_gap[1][0].is_list_item);
        assert!(!over_gap[1][8].is_list_item);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_empty_paragraphs_do_not_consume_ordered_list_gap() {
        const EMPTY_PARAGRAPHS: usize = 12;
        const ORIGINAL_THIRD_ITEM_INDEX: usize = 8;
        let mut pages = anchored_ordered_list_test_pages();
        for _ in 0..EMPTY_PARAGRAPHS {
            pages[1].insert(1, ordered_list_test_paragraph(" \n "));
        }

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(pages[1][0].is_list_item);
        assert!(pages[1][ORIGINAL_THIRD_ITEM_INDEX + EMPTY_PARAGRAPHS].is_list_item);
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejects_structural_ordered_list_candidates() {
        for structural_kind in ["heading", "list", "code", "formula", "furniture", "caption"] {
            let mut pages = anchored_ordered_list_test_pages();
            let candidate = &mut pages[1][0];
            match structural_kind {
                "heading" => candidate.heading_level = Some(2),
                "list" => candidate.is_list_item = true,
                "code" => candidate.is_code_block = true,
                "formula" => candidate.is_formula = true,
                "furniture" => candidate.is_page_furniture = true,
                "caption" => candidate.layout_class = Some(types::LayoutHintClass::Caption),
                _ => unreachable!("all test cases are handled"),
            }

            promote_anchored_ordered_list_sequences(&mut pages);

            assert!(!pages[1][8].is_list_item, "structural kind: {structural_kind}");
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_rejects_numbered_section_heading_candidates() {
        let mut pages = anchored_ordered_list_test_pages();
        pages[1][0].text = "2. DEFINITIONS".to_string();
        pages[1][8].text = "3. TERM".to_string();

        promote_anchored_ordered_list_sequences(&mut pages);

        assert!(!pages[1][0].is_list_item);
        assert!(!pages[1][8].is_list_item);
    }
}
