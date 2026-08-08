//! Layout-aware OCR table recognition.
//!
//! This module provides TATR-based table structure recognition for OCR pages.
//! It operates entirely in pixel space — no coordinate conversion is needed
//! because both OCR elements and layout detections use the same image
//! coordinate system (origin top-left, y increases downward).

use crate::layout::models::tatr::{self, TatrModel};
use crate::layout::types::{BBox, DetectionResult, LayoutClass, RecognizedTable};
use crate::types::{OcrElement, OcrElementLevel};

/// Default confidence threshold for layout detections.
const MIN_CONFIDENCE: f32 = 0.3;

/// Minimum intersection-over-word-area required to assign an OCR element to a table cell.
const MIN_CELL_ELEMENT_IOW: f32 = 0.2;

/// Run TATR table recognition for all Table regions in a page.
///
/// For each Table detection, crops the page image, runs TATR inference,
/// matches OCR elements to cells, and produces markdown tables.
pub(crate) fn recognize_page_tables(
    page_image: &image::RgbImage,
    detection: &DetectionResult,
    elements: &[OcrElement],
    tatr_model: &mut TatrModel,
) -> Vec<RecognizedTable> {
    let mut tables = Vec::new();

    for det in &detection.detections {
        if det.class_name != LayoutClass::Table || det.confidence < MIN_CONFIDENCE {
            continue;
        }

        let result = recognize_single_table(page_image, &det.bbox, elements, tatr_model);
        if let Some((cells, markdown)) = result {
            tables.push(RecognizedTable {
                detection_bbox: det.bbox,
                cells,
                markdown,
            });
        }
    }

    tables
}

/// Recognize a single table from a cropped region of the page.
///
/// Returns `(cells, markdown)` where cells is the 2D grid of cell text content.
fn recognize_single_table(
    page_image: &image::RgbImage,
    table_bbox: &BBox,
    elements: &[OcrElement],
    tatr_model: &mut TatrModel,
) -> Option<(Vec<Vec<String>>, String)> {
    let crop_x = table_bbox.x1.max(0.0) as u32;
    let crop_y = table_bbox.y1.max(0.0) as u32;
    let crop_w = (table_bbox.width() as u32).min(page_image.width().saturating_sub(crop_x));
    let crop_h = (table_bbox.height() as u32).min(page_image.height().saturating_sub(crop_y));

    if crop_w == 0 || crop_h == 0 {
        return None;
    }

    let cropped = image::imageops::crop_imm(page_image, crop_x, crop_y, crop_w, crop_h).to_image();

    let tatr_result = match tatr_model.recognize(&cropped) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("TATR inference failed: {e}");
            return None;
        }
    };

    if tatr_result.rows.is_empty() || tatr_result.columns.is_empty() {
        return None;
    }

    let crop_table_bbox = effective_table_bbox(tatr_result.table_bbox, crop_w as f32, crop_h as f32);
    let (cell_grid, structure) = tatr::build_cell_grid_with_structure(&tatr_result, Some(crop_table_bbox));
    if cell_grid.is_empty() || cell_grid[0].is_empty() {
        return None;
    }

    if !is_cell_grid_valid(&cell_grid) {
        tracing::debug!("TATR cell grid is invalid (too many empty cells or malformed); skipping table");
        return None;
    }

    let table_elements = select_table_elements(elements, table_bbox);

    let (cells, markdown) = build_markdown_table(&cell_grid, &table_elements, crop_x as f32, crop_y as f32, &structure);
    Some((cells, markdown))
}

/// Choose the crop-local bounding box used to widen table columns to the
/// table's full horizontal extent: prefer TATR's own detected `Table` region
/// (already in crop-pixel coordinates); otherwise fall back to the full crop
/// extent, which was itself sized from the layout detector's table bbox.
///
/// Previously this call site always passed `None`, so column widening was
/// inferred purely from row detections, clipping any column that extended
/// past the outermost detected row (xberg-io/xberg#193).
fn effective_table_bbox(tatr_table_bbox: Option<[f32; 4]>, crop_w: f32, crop_h: f32) -> [f32; 4] {
    tatr_table_bbox.unwrap_or([0.0, 0.0, crop_w, crop_h])
}

fn select_table_elements<'a>(elements: &'a [OcrElement], table_bbox: &BBox) -> Vec<&'a OcrElement> {
    let mut words = Vec::new();
    let mut lines = Vec::new();
    let mut blocks = Vec::new();

    for element in elements {
        if element.text.trim().is_empty() || element_bbox_iow(element, table_bbox) < MIN_CELL_ELEMENT_IOW {
            continue;
        }

        match element.level {
            OcrElementLevel::Word => words.push(element),
            OcrElementLevel::Line => lines.push(element),
            // Block is a legitimate fallback: some backends only emit block-level
            // geometry. Previously it was dropped outright, so a table region
            // whose OCR output has no Word/Line elements produced an empty grid
            // instead of falling back further (xberg-io/xberg#193). Page-level
            // elements remain excluded — they span the whole page and cannot be
            // usefully assigned to a single cell.
            OcrElementLevel::Block => blocks.push(element),
            OcrElementLevel::Page => {}
        }
    }

    if !words.is_empty() {
        words
    } else if !lines.is_empty() {
        lines
    } else {
        blocks
    }
}

/// Build a markdown table from TATR cell grid + OCR elements.
///
/// Cell bboxes from TATR are in cropped-image coordinates.
/// OCR elements are in page coordinates. `offset_x/y` translates between them.
fn build_markdown_table(
    cell_grid: &[Vec<tatr::CellBBox>],
    elements: &[&OcrElement],
    offset_x: f32,
    offset_y: f32,
    structure: &tatr::TableStructure,
) -> (Vec<Vec<String>>, String) {
    if cell_grid.is_empty() {
        return (Vec::new(), String::new());
    }

    let num_cols = cell_grid[0].len();

    if num_cols == 0 {
        return (Vec::new(), String::new());
    }

    let mut assigned = assign_elements_to_best_cells(cell_grid, elements, offset_x, offset_y, num_cols);
    let mut grid: Vec<Vec<String>> = Vec::with_capacity(assigned.len());
    for row in &mut assigned {
        let mut grid_row = vec![String::new(); num_cols];
        for (column, cell_elements) in row.iter_mut().enumerate() {
            grid_row[column] = text_from_assigned_elements(cell_elements);
        }
        grid.push(grid_row);
    }

    merge_spanning_cells(&mut grid, &structure.spans);

    // TATR's own header rows are the ground truth for where the separator
    // belongs; default to "row 0 only" when it detected none, matching the
    // prior hardcoded behavior (xberg-io/xberg#176).
    let header_rows = structure.header_row_count.max(1).min(grid.len());

    let mut md = String::new();

    for (row_idx, row) in grid.iter().enumerate() {
        md.push('|');
        for cell in row {
            let escaped = cell.replace('|', "\\|");
            md.push(' ');
            md.push_str(escaped.trim());
            md.push_str(" |");
        }
        md.push('\n');

        if row_idx + 1 == header_rows {
            md.push('|');
            for _ in 0..num_cols {
                md.push_str(" --- |");
            }
            md.push('\n');
        }
    }

    if md.ends_with('\n') {
        md.pop();
    }

    (grid, md)
}

/// Merge each TATR spanning-cell region into its top-left grid cell and blank
/// the rest of the covered cells.
///
/// TATR's cell grid is built as the rectangular row∩column intersection of
/// independently detected rows/columns, so a single merged source cell (e.g.
/// a two-column header) is split across several grid cells and any OCR
/// element inside it is assigned to whichever cell it geometrically falls
/// in — splitting one label into fragments across neighboring cells
/// (xberg-io/xberg#176). Concatenating the covered cells' text back together
/// restores the original single-cell content.
fn merge_spanning_cells(grid: &mut [Vec<String>], spans: &[(usize, usize, usize, usize)]) {
    for &(row_start, row_end, col_start, col_end) in spans {
        if grid.is_empty() || row_end > grid.len() || col_end > grid[0].len() {
            continue;
        }
        let mut combined: Vec<String> = Vec::new();
        for row in grid.iter_mut().take(row_end).skip(row_start) {
            for cell in row.iter_mut().take(col_end).skip(col_start) {
                if !cell.is_empty() {
                    combined.push(std::mem::take(cell));
                }
            }
        }
        grid[row_start][col_start] = combined.join(" ");
    }
}

type PositionedElement<'a> = (&'a OcrElement, f32, f32);
type CellAssignments<'a> = Vec<Vec<Vec<PositionedElement<'a>>>>;

/// Assign every OCR element to its single highest-IoW cell across the full grid.
/// Elements without cell overlap use the nearest cell. Equal scores keep the
/// first cell in row-major order.
fn assign_elements_to_best_cells<'a>(
    cell_grid: &[Vec<tatr::CellBBox>],
    elements: &[&'a OcrElement],
    offset_x: f32,
    offset_y: f32,
    num_cols: usize,
) -> CellAssignments<'a> {
    let page_cells = cell_grid
        .iter()
        .map(|row| {
            row.iter()
                .take(num_cols)
                .map(|cell| {
                    BBox::new(
                        cell.x1 + offset_x,
                        cell.y1 + offset_y,
                        cell.x2 + offset_x,
                        cell.y2 + offset_y,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut assigned: CellAssignments<'a> = page_cells.iter().map(|row| vec![Vec::new(); row.len()]).collect();

    for &element in elements {
        let mut best_iow = 0.0;
        let mut best_cell = None;
        let mut nearest_distance = f32::INFINITY;
        let mut nearest_cell = None;
        let (center_x, center_y) = element_center_f32(element);
        for (row, cells) in page_cells.iter().enumerate() {
            for (column, cell) in cells.iter().enumerate() {
                let iow = element_bbox_iow(element, cell);
                if iow > best_iow {
                    best_iow = iow;
                    best_cell = Some((row, column));
                }
                let distance = point_to_bbox_distance_squared(center_x, center_y, cell);
                if distance < nearest_distance {
                    nearest_distance = distance;
                    nearest_cell = Some((row, column));
                }
            }
        }
        if let Some((row, column)) = best_cell.or(nearest_cell) {
            assigned[row][column].push((element, center_x, center_y));
        }
    }
    assigned
}

/// Assemble uniquely assigned elements in top-to-bottom, left-to-right order.
fn text_from_assigned_elements(elements: &mut [PositionedElement<'_>]) -> String {
    if elements.is_empty() {
        return String::new();
    }
    elements.sort_by(|left, right| left.2.total_cmp(&right.2).then_with(|| left.1.total_cmp(&right.1)));
    elements
        .iter()
        .map(|(element, _, _)| element.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compute intersection-over-word-area (IoW) between an OCR element and a BBox.
///
/// Returns the fraction of the element's area that overlaps with the given bbox.
/// For zero-area elements, falls back to center-point containment (returns 0.0 or 1.0).
fn element_bbox_iow(elem: &OcrElement, bbox: &BBox) -> f32 {
    let (left, top, width, height) = elem.geometry.to_aabb();
    let e_left = left as f32;
    let e_top = top as f32;
    let e_right = e_left + width as f32;
    let e_bottom = e_top + height as f32;
    let elem_area = width as f32 * height as f32;

    if elem_area <= 0.0 {
        let cx = e_left + width as f32 / 2.0;
        let cy = e_top + height as f32 / 2.0;
        return if point_in_bbox(cx, cy, bbox) { 1.0 } else { 0.0 };
    }

    let inter_left = e_left.max(bbox.x1);
    let inter_top = e_top.max(bbox.y1);
    let inter_right = e_right.min(bbox.x2);
    let inter_bottom = e_bottom.min(bbox.y2);
    let inter_area = (inter_right - inter_left).max(0.0) * (inter_bottom - inter_top).max(0.0);

    inter_area / elem_area
}

/// Get element center as f32 (for matching with BBox which uses f32).
fn element_center_f32(elem: &OcrElement) -> (f32, f32) {
    let (cx, cy) = elem.geometry.center();
    (cx as f32, cy as f32)
}

/// Check if a point (cx, cy) is inside a BBox (pixel coords: y increases downward).
fn point_in_bbox(cx: f32, cy: f32, bbox: &BBox) -> bool {
    cx >= bbox.x1 && cx <= bbox.x2 && cy >= bbox.y1 && cy <= bbox.y2
}

fn point_to_bbox_distance_squared(x: f32, y: f32, bbox: &BBox) -> f32 {
    let horizontal = if x < bbox.x1 {
        bbox.x1 - x
    } else if x > bbox.x2 {
        x - bbox.x2
    } else {
        0.0
    };
    let vertical = if y < bbox.y1 {
        bbox.y1 - y
    } else if y > bbox.y2 {
        y - bbox.y2
    } else {
        0.0
    };
    horizontal * horizontal + vertical * vertical
}

/// Validate TATR cell grid sanity.
///
/// Detects malformed tables from low-confidence TATR output (category C).
/// Returns false if:
/// - More than 30% of cells are empty (indicates bad segmentation)
/// - Grid has < 2 rows or < 2 columns (degenerate)
fn is_cell_grid_valid(cell_grid: &[Vec<tatr::CellBBox>]) -> bool {
    if cell_grid.len() < 2 {
        return false;
    }
    if cell_grid[0].len() < 2 {
        return false;
    }

    let mut empty_count = 0;
    let total_count = cell_grid.len() * cell_grid[0].len();

    for row in cell_grid {
        for cell in row {
            let width = (cell.x2 - cell.x1).abs();
            let height = (cell.y2 - cell.y1).abs();
            if width < 1.0 || height < 1.0 {
                empty_count += 1;
            }
        }
    }

    let empty_ratio = empty_count as f32 / total_count as f32;
    if empty_ratio > 0.3 {
        tracing::debug!(
            empty_count,
            total_count,
            empty_ratio,
            "TATR cell grid has too many empty cells ({:.1}%)",
            empty_ratio * 100.0
        );
        return false;
    }

    true
}

#[cfg(all(test, feature = "ocr"))]
mod tests {
    use super::*;
    use crate::types::{OcrBoundingGeometry, OcrConfidence, OcrElementLevel};

    fn cell(x1: f32, y1: f32, x2: f32, y2: f32) -> tatr::CellBBox {
        tatr::CellBBox { x1, y1, x2, y2 }
    }

    /// A `TableStructure` carrying no header/span information, matching what
    /// `build_cell_grid_with_structure` returns when TATR detected neither.
    fn no_structure() -> tatr::TableStructure {
        tatr::TableStructure {
            header_row_count: 0,
            spans: Vec::new(),
        }
    }

    fn word(text: &str, left: u32, top: u32, width: u32, height: u32) -> OcrElement {
        OcrElement::new(
            text,
            OcrBoundingGeometry::Rectangle {
                left,
                top,
                width,
                height,
            },
            OcrConfidence::from_tesseract(95.0),
        )
        .with_level(OcrElementLevel::Word)
    }

    fn line(text: &str, left: u32, top: u32, width: u32, height: u32) -> OcrElement {
        OcrElement::new(
            text,
            OcrBoundingGeometry::Rectangle {
                left,
                top,
                width,
                height,
            },
            OcrConfidence::from_tesseract(95.0),
        )
        .with_level(OcrElementLevel::Line)
    }

    #[test]
    fn should_prefer_valid_words_over_lines_for_table_assignment() {
        let table_bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        let elements = [
            line("duplicated line", 10, 10, 80, 10),
            word("first", 10, 10, 20, 10),
            word("second", 40, 10, 25, 10),
            line("block", 10, 30, 80, 10).with_level(OcrElementLevel::Block),
            word("", 70, 10, 10, 10),
            word("outside", 150, 10, 20, 10),
        ];

        let selected = select_table_elements(&elements, &table_bbox);
        let selected_text = selected.iter().map(|element| element.text.as_str()).collect::<Vec<_>>();

        assert_eq!(selected_text, vec!["first", "second"]);
    }

    #[test]
    fn should_fall_back_to_valid_lines_when_table_has_no_valid_words() {
        let table_bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        let elements = [line("line text", 10, 10, 80, 10), word("outside", 150, 10, 20, 10)];

        let selected = select_table_elements(&elements, &table_bbox);
        let selected_text = selected.iter().map(|element| element.text.as_str()).collect::<Vec<_>>();

        assert_eq!(selected_text, vec!["line text"]);
    }

    #[test]
    fn should_fall_back_to_valid_blocks_when_table_has_no_words_or_lines() {
        let table_bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        let block = line("block text", 10, 10, 80, 10).with_level(OcrElementLevel::Block);
        let elements = [block, word("outside", 150, 10, 20, 10)];

        let selected = select_table_elements(&elements, &table_bbox);
        let selected_text = selected.iter().map(|element| element.text.as_str()).collect::<Vec<_>>();

        assert_eq!(
            selected_text,
            vec!["block text"],
            "a table region with only block-level OCR output must not yield an empty cell grid (#193)"
        );
    }

    #[test]
    fn should_prefer_tatr_detected_table_bbox_over_full_crop_extent() {
        let bbox = effective_table_bbox(Some([5.0, 5.0, 95.0, 45.0]), 100.0, 50.0);
        assert_eq!(bbox, [5.0, 5.0, 95.0, 45.0]);
    }

    #[test]
    fn should_fall_back_to_full_crop_extent_when_tatr_has_no_table_detection() {
        let bbox = effective_table_bbox(None, 100.0, 50.0);
        assert_eq!(
            bbox,
            [0.0, 0.0, 100.0, 50.0],
            "must widen to the full crop instead of the previous hardcoded None (#193)"
        );
    }

    #[test]
    fn should_assign_ordinary_grid_elements_in_reading_order() {
        let grid = vec![
            vec![cell(0.0, 0.0, 50.0, 50.0), cell(50.0, 0.0, 100.0, 50.0)],
            vec![cell(0.0, 50.0, 50.0, 100.0), cell(50.0, 50.0, 100.0, 100.0)],
        ];
        let elements = [
            word("B", 70, 10, 10, 10),
            word("two", 25, 10, 10, 10),
            word("one", 10, 10, 10, 10),
            word("D", 70, 70, 10, 10),
            word("C", 10, 70, 10, 10),
        ];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, markdown) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["one two", "B"], vec!["C", "D"]]);
        assert_eq!(markdown, "| one two | B |\n| --- | --- |\n| C | D |");
    }

    #[test]
    fn should_assign_overlapping_element_only_to_highest_iow_cell() {
        let grid = vec![vec![cell(0.0, 0.0, 60.0, 50.0), cell(40.0, 0.0, 100.0, 50.0)]];
        let elements = [word("overlap", 50, 10, 20, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["", "overlap"]]);
    }

    #[test]
    fn should_break_equal_iow_ties_by_row_then_column() {
        let grid = vec![vec![cell(0.0, 0.0, 60.0, 50.0), cell(40.0, 0.0, 100.0, 50.0)]];
        let elements = [word("tie", 45, 10, 10, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["tie", ""]]);
    }

    #[test]
    fn should_assign_zero_area_element_by_center_without_duplication() {
        let grid = vec![vec![cell(0.0, 0.0, 50.0, 50.0), cell(50.0, 0.0, 100.0, 50.0)]];
        let elements = [word("point", 75, 25, 0, 0)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["", "point"]]);
    }

    #[test]
    fn should_emit_spanning_cell_element_once_for_repeated_boxes() {
        let spanning = cell(0.0, 0.0, 100.0, 50.0);
        let grid = vec![vec![spanning, spanning]];
        let elements = [word("span", 40, 10, 20, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["span", ""]]);
    }

    #[test]
    fn should_assign_low_iow_element_to_best_overlapping_cell() {
        let grid = vec![vec![cell(0.0, 0.0, 40.0, 50.0), cell(60.0, 0.0, 100.0, 50.0)]];
        let elements = [word("edge", 30, 10, 400, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["", "edge"]]);
    }

    #[test]
    fn should_assign_non_overlapping_element_to_nearest_cell_once() {
        let grid = vec![vec![cell(0.0, 0.0, 40.0, 50.0), cell(60.0, 0.0, 100.0, 50.0)]];
        let elements = [word("gap", 45, 10, 10, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["gap", ""]]);
        assert_eq!(cells.iter().flatten().filter(|text| text.contains("gap")).count(), 1);
    }

    #[test]
    fn should_preserve_duplicate_word_multiset_without_multiplying_assignments() {
        let grid = vec![vec![cell(0.0, 0.0, 40.0, 50.0), cell(60.0, 0.0, 100.0, 50.0)]];
        let elements = [word("total", 30, 10, 400, 10), word("total", 30, 20, 400, 10)];
        let element_refs = elements.iter().collect::<Vec<_>>();

        let (cells, _) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &no_structure());

        assert_eq!(cells, vec![vec!["", "total total"]]);
        assert_eq!(
            cells.iter().flatten().flat_map(|cell| cell.split_whitespace()).count(),
            2
        );
    }

    #[test]
    fn should_merge_spanning_cell_content_instead_of_splitting_across_columns() {
        // A 2-column header ("Quarterly Report") is split into two grid cells
        // by TATR's row∩column intersection; the structure says (row 0, cols
        // 0..2) is one spanning cell.
        let grid = vec![
            vec![cell(0.0, 0.0, 50.0, 20.0), cell(50.0, 0.0, 100.0, 20.0)],
            vec![cell(0.0, 20.0, 50.0, 40.0), cell(50.0, 20.0, 100.0, 40.0)],
        ];
        let elements = [
            word("Quarterly", 10, 5, 30, 10),
            word("Report", 60, 5, 30, 10),
            word("Q1", 10, 25, 10, 10),
            word("Q2", 60, 25, 10, 10),
        ];
        let element_refs = elements.iter().collect::<Vec<_>>();
        let structure = tatr::TableStructure {
            header_row_count: 1,
            spans: vec![(0, 1, 0, 2)],
        };

        let (cells, markdown) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &structure);

        assert_eq!(
            cells,
            vec![vec!["Quarterly Report", ""], vec!["Q1", "Q2"]],
            "spanning cell content must be merged into the origin cell, not split (#176)"
        );
        assert_eq!(markdown, "| Quarterly Report |  |\n| --- | --- |\n| Q1 | Q2 |");
    }

    #[test]
    fn should_place_header_separator_after_all_tatr_detected_header_rows() {
        let grid = vec![
            vec![cell(0.0, 0.0, 50.0, 20.0), cell(50.0, 0.0, 100.0, 20.0)],
            vec![cell(0.0, 20.0, 50.0, 40.0), cell(50.0, 20.0, 100.0, 40.0)],
            vec![cell(0.0, 40.0, 50.0, 60.0), cell(50.0, 40.0, 100.0, 60.0)],
        ];
        let elements = [
            word("Title", 10, 5, 10, 10),
            word("Region", 60, 5, 10, 10),
            word("Sub", 10, 25, 10, 10),
            word("Head", 60, 25, 10, 10),
            word("A", 10, 45, 10, 10),
            word("B", 60, 45, 10, 10),
        ];
        let element_refs = elements.iter().collect::<Vec<_>>();
        let structure = tatr::TableStructure {
            header_row_count: 2,
            spans: Vec::new(),
        };

        let (_, markdown) = build_markdown_table(&grid, &element_refs, 0.0, 0.0, &structure);

        assert_eq!(
            markdown, "| Title | Region |\n| Sub | Head |\n| --- | --- |\n| A | B |",
            "the separator must follow both TATR-detected header rows, not just row 0 (#176)"
        );
    }
}
