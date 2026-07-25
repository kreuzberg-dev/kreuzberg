//! Native table detection using the pdf_oxide backend.
//!
//! Three entry points:
//!
//! - [`extract_tables_native`] — pdf_oxide's built-in `extract_tables_with_config`
//!   in strict mode. High precision, requires explicit table grid with 3+ columns.
//! - [`extract_tables_bordered`] — relaxed Lines-strategy pass for bordered tables
//!   with 2+ columns. Catches 2-column data tables (label/value) whose cells are
//!   delimited by stroke lines that `strict()` skips due to `min_table_columns=3`.
//! - [`extract_tables_heuristic`] — text-layer fallback that reuses the same
//!   spatial reconstruction the layout-detection path uses, but without
//!   requiring layout hints. Catches tables that pdf_oxide's grid detector
//!   misses (e.g. invoice line items, financial tables without ruling lines).
//!
//! The default extraction flow in `extractors::pdf::extraction` runs all three
//! per-page in priority order (native → bordered → heuristic), where each tier
//! only runs on pages where the previous tier found nothing.

use super::OxideDocument;
use crate::pdf::error::{PdfError, Result};
use crate::types::{BoundingBox, Table};
use std::collections::HashSet;

/// Cap on candidate vertical regions per page. Real tables fit comfortably
/// under this; prose-heavy pages can otherwise generate dozens of small
/// regions that each go through `reconstruct_table` + `post_process_table`,
/// burning CPU on guaranteed rejections. The cap is generous on purpose —
/// the validation chain is fast.
const MAX_REGIONS_PER_PAGE: usize = 20;
const DENSE_NUMERIC_MIN_RECURRING_ROWS: usize = 5;
const DENSE_NUMERIC_MIN_WORDS_PER_ROW: usize = 3;
const DENSE_NUMERIC_MIN_ROW_WORD_PERCENT: usize = 60;
const DENSE_NUMERIC_MIN_RECURRING_TRACKS: usize = 4;
const DENSE_NUMERIC_MIN_TRACK_ROW_PERCENT: usize = 60;
const NUMERIC_HEADER_MIN_TRACK_PERCENT: usize = 60;
const NUMERIC_HEADER_MIN_ALPHA_PERCENT: usize = 60;
const NUMERIC_HEADER_MAX_ROWS: usize = 2;
const DENSE_NUMERIC_COLUMN_GAP_CAP: u32 = 20;
const SPLIT_NUMERIC_TRACK_MIN_ROWS_PER_SIDE: usize = 2;
const SPLIT_NUMERIC_TRACK_MIN_TOTAL_ROWS: usize = 6;
const SIDE_BY_SIDE_MIN_PARENT_COLUMNS: usize = 7;
const SIDE_BY_SIDE_CENTER_TOLERANCE_PERCENT: u64 = 8;
const SIDE_BY_SIDE_MIN_GUTTER_HEIGHT_PERCENT: u64 = 100;
const SIDE_BY_SIDE_MIN_WIDTH_PERCENT: u64 = 35;
const SIDE_BY_SIDE_MIN_WORDS_PER_SIDE: usize = 6;
const SIDE_BY_SIDE_CHILD_GAP_HEIGHT_MULTIPLIER: u32 = 6;
const SIDE_BY_SIDE_MIN_TRACK_ROW_PERCENT: usize = 20;
const SIDE_BY_SIDE_MIN_NUMERIC_TRACKS: usize = 2;
const SIDE_BY_SIDE_MAX_TRACK_DELTA: usize = 1;
const WRAPPED_FINANCIAL_COLUMNS: usize = 4;
const WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS: usize = 2;
const WRAPPED_FINANCIAL_MAX_CONTINUATION_ROWS: usize = 6;
const WRAPPED_FINANCIAL_MIN_CONTINUATION_ROWS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SideTableShape {
    columns: usize,
    numeric_tracks: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeuristicTableRejection {
    EmptyGrid,
    PostProcessing,
    TooFewRows,
    CodeListing,
    NotWellFormed,
    EmptyMarkdown,
}

/// Extract tables from all pages using pdf_oxide's native table detection.
///
/// Uses `TableDetectionConfig::strict()` to reduce false positives from
/// paragraph text being misidentified as tables.
///
/// # Arguments
///
/// * `doc` - Mutable reference to the oxide document
///
/// # Returns
///
/// A `Vec<Table>` containing all detected tables with cells, markdown, and bounding boxes.
pub(crate) fn extract_tables_native(doc: &mut OxideDocument) -> Result<Vec<Table>> {
    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("pdf_oxide: failed to get page count: {e}")))?;

    let config = pdf_oxide::structure::spatial_table_detector::TableDetectionConfig::strict();
    let mut all_tables = Vec::new();

    for page_idx in 0..page_count {
        let extracted = match doc.doc.extract_tables_with_config(page_idx, config.clone()) {
            Ok(tables) => tables,
            Err(e) => {
                tracing::warn!(page = page_idx, "pdf_oxide extract_tables failed: {e}");
                continue;
            }
        };

        let page_number = (page_idx + 1) as u32;

        for extracted_table in extracted {
            if extracted_table.rows.is_empty() || extracted_table.col_count == 0 {
                continue;
            }

            let (cells, markdown) = convert_extracted_table(&extracted_table);

            if cells.is_empty() || markdown.trim().is_empty() {
                continue;
            }

            if cells.len() < 2 || cells.iter().all(|row| row.len() < 2) {
                tracing::debug!(
                    page = page_idx,
                    rows = cells.len(),
                    cols = cells.first().map(|r| r.len()).unwrap_or(0),
                    "Skipping table below minimum dimensions (need ≥2 rows and ≥2 cols)"
                );
                continue;
            }

            let bounding_box = extracted_table.bbox.map(|rect| BoundingBox {
                x0: rect.x as f64,
                y0: rect.y as f64,
                x1: (rect.x + rect.width) as f64,
                y1: (rect.y + rect.height) as f64,
            });

            all_tables.push(Table {
                cells,
                markdown,
                page_number,
                bounding_box,
                ..Default::default()
            });
        }
    }

    Ok(all_tables)
}

/// Extract bordered tables from all pages using a relaxed Lines-strategy config.
///
/// Targets 2-column tables whose cells are delimited by stroke/fill rectangles
/// (the `re` PDF operator or explicit `m/l` paths) that `extract_tables_native`
/// misses because `strict()` requires ≥ 3 columns.
///
/// Uses `TableStrategy::Lines` for both axes — path primitives only, no text-
/// edge heuristics — so this pass does not introduce new text-based false positives.
/// The relaxed thresholds:
///
/// - `min_table_columns = 2` (vs 3 in strict)
/// - `min_table_cells = 4` (vs 6 in strict)
/// - `regular_row_ratio = 0.5` (vs 0.8 in strict)
///
/// `skip_pages` (1-indexed) suppresses this pass on pages where a higher-priority
/// detector already produced a result. Pass an empty set to run on every page.
pub(crate) fn extract_tables_bordered(doc: &mut OxideDocument, skip_pages: &HashSet<u32>) -> Result<Vec<Table>> {
    use pdf_oxide::structure::spatial_table_detector::{TableDetectionConfig, TableStrategy};

    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("pdf_oxide: failed to get page count: {e}")))?;

    let config = TableDetectionConfig {
        enabled: true,
        horizontal_strategy: TableStrategy::Lines,
        vertical_strategy: TableStrategy::Lines,
        column_tolerance: 3.0,
        row_tolerance: 2.0,
        min_table_cells: 4,
        min_table_columns: 2,
        regular_row_ratio: 0.5,
        max_table_columns: 15,
        column_merge_threshold: 12.0,
        v_split_gap: 4.0,
        text_fallback: false,
    };

    let mut all_tables = Vec::new();

    for page_idx in 0..page_count {
        let page_number = (page_idx + 1) as u32;
        if skip_pages.contains(&page_number) {
            continue;
        }

        let extracted = match doc.doc.extract_tables_with_config(page_idx, config.clone()) {
            Ok(tables) => tables,
            Err(e) => {
                tracing::warn!(page = page_idx, "pdf_oxide bordered extract_tables failed: {e}");
                continue;
            }
        };

        for extracted_table in extracted {
            if extracted_table.rows.is_empty() || extracted_table.col_count == 0 {
                continue;
            }

            let (cells, markdown) = convert_extracted_table(&extracted_table);

            if cells.is_empty() || markdown.trim().is_empty() {
                continue;
            }

            if cells.len() < 2 || cells.iter().all(|row| row.len() < 2) {
                tracing::debug!(
                    page = page_idx,
                    rows = cells.len(),
                    cols = cells.first().map(|r| r.len()).unwrap_or(0),
                    "Skipping bordered table below minimum dimensions"
                );
                continue;
            }

            let bounding_box = extracted_table.bbox.map(|rect| BoundingBox {
                x0: rect.x as f64,
                y0: rect.y as f64,
                x1: (rect.x + rect.width) as f64,
                y1: (rect.y + rect.height) as f64,
            });

            all_tables.push(Table {
                cells,
                markdown,
                page_number,
                bounding_box,
                ..Default::default()
            });
        }
    }

    Ok(all_tables)
}

/// Owned hierarchy segments produced while detecting heuristic tables.
///
/// Structured PDF rendering can consume these segments directly instead of
/// extracting the same hierarchy a second time.
pub(crate) struct ExtractedHierarchySegments {
    pub(crate) pages: Vec<Vec<crate::pdf::hierarchy::SegmentData>>,
    pub(crate) used_structure_tree: bool,
}

/// Heuristic tables and the hierarchy segments used to derive them.
pub(crate) struct HeuristicTableExtraction {
    pub(crate) tables: Vec<Table>,
    pub(crate) hierarchy_segments: ExtractedHierarchySegments,
}

/// Heuristic table reconstruction for text-layer PDFs without layout hints.
///
/// Pulls per-page text segments via the same hierarchy extractor used by the
/// structured pipeline, projects each segment into an `HocrWord`, clusters
/// words into vertically-contiguous regions by abnormally-large row gaps,
/// then runs the shared `reconstruct_table` → `post_process_table` →
/// `is_well_formed_table` chain (the same one consumed by the OCR pipeline
/// and the layout-detection path) per region.
///
/// The clustering step does NOT validate that a region is table-shaped — it
/// merely separates vertically-isolated word slabs. Multi-column prose on a
/// single page will pass clustering as one wide region; `is_well_formed_table`
/// is the actual prose-rejection guard. We pass `layout_guided = true` to
/// `post_process_table` so that "we already pre-segmented the page" earns
/// the relaxed text-density thresholds, but the prose-detection check in
/// `is_well_formed_table` (row coherence + column semantic uniformity) is
/// what catches columned articles and similar false positives.
///
/// `skip_pages` (1-indexed, matching `Table.page_number`) lets the caller
/// suppress heuristic work on pages where pdf_oxide's native grid detector
/// already produced results — those tables are higher precision than
/// anything the heuristic can derive, so we keep native and only fill in
/// the gaps. Pass an empty set to run on every page.
///
/// This is invoked by `extractors::pdf::extraction` as a per-page fallback
/// alongside [`extract_tables_native`] — typically for text-layer PDFs
/// whose tables aren't drawn with explicit rule lines that pdf_oxide's grid
/// detector can lock onto.
///
/// Returns the detected tables together with ownership of the exact hierarchy
/// segments used for reconstruction.
pub(crate) fn extract_tables_heuristic(
    doc: &mut OxideDocument,
    allow_single_column: bool,
    skip_pages: &HashSet<u32>,
) -> Result<HeuristicTableExtraction> {
    use crate::pdf::table_reconstruct::{HocrWord, segments_to_words};

    let (per_page_segments, used_structure_tree) =
        crate::pdf::oxide::hierarchy::extract_all_segments(doc).map_err(|e| {
            PdfError::TextExtractionFailed(format!(
                "pdf_oxide hierarchy extraction failed for heuristic tables: {e}"
            ))
        })?;

    let page_count = doc
        .doc
        .page_count()
        .map_err(|e| PdfError::MetadataExtractionFailed(format!("pdf_oxide: failed to get page count: {e}")))?;

    let mut tables = Vec::new();

    for page_idx in 0..page_count {
        let page_number = (page_idx + 1) as u32;
        if skip_pages.contains(&page_number) {
            continue;
        }

        let Some(segments) = per_page_segments.get(page_idx) else {
            continue;
        };
        if segments.is_empty() {
            continue;
        }

        let page_height = segments
            .iter()
            .map(|s| s.y + s.height)
            .fold(0.0_f32, f32::max)
            .max(792.0);

        let words: Vec<HocrWord> = segments_to_words(segments, page_height);
        if words.len() < 4 {
            continue;
        }

        let mut regions = cluster_words_into_vertical_regions(&words);
        if regions.len() > MAX_REGIONS_PER_PAGE {
            tracing::debug!(
                page = page_number,
                regions = regions.len(),
                cap = MAX_REGIONS_PER_PAGE,
                "heuristic table extraction: capping candidate regions on this page",
            );
            regions.truncate(MAX_REGIONS_PER_PAGE);
        }

        for region in regions {
            tables.extend(reconstruct_region_tables(
                &region,
                page_height,
                page_number,
                allow_single_column,
            ));
        }
    }

    Ok(HeuristicTableExtraction {
        tables,
        hierarchy_segments: ExtractedHierarchySegments {
            pages: per_page_segments,
            used_structure_tree,
        },
    })
}

fn reconstruct_region_tables(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    page_height: f32,
    page_number: u32,
    allow_single_column: bool,
) -> Vec<Table> {
    let Some(parent) = reconstruct_region_table(region, page_height, page_number, allow_single_column) else {
        return Vec::new();
    };
    if parent.cells.first().map_or(0, Vec::len) < SIDE_BY_SIDE_MIN_PARENT_COLUMNS {
        return vec![parent];
    }

    let Some((left_region, right_region)) = split_side_by_side_region(region) else {
        return vec![parent];
    };
    let Some(left) = reconstruct_side_by_side_child(&left_region, page_height, page_number, allow_single_column) else {
        return vec![parent];
    };
    let Some(right) = reconstruct_side_by_side_child(&right_region, page_height, page_number, allow_single_column)
    else {
        return vec![parent];
    };
    if !side_tables_have_independent_shape(&left, &right) {
        return vec![parent];
    }

    let (left, right) = normalize_side_by_side_financial_tables(left, right);
    tracing::trace!(
        page = page_number,
        parent_columns = parent.cells.first().map_or(0, Vec::len),
        left_columns = left.cells.first().map_or(0, Vec::len),
        right_columns = right.cells.first().map_or(0, Vec::len),
        "split independently valid side-by-side heuristic tables"
    );
    vec![left, right]
}

fn reconstruct_side_by_side_child(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    page_height: f32,
    page_number: u32,
    allow_single_column: bool,
) -> Option<Table> {
    let mut heights: Vec<u32> = region.iter().map(|word| word.height).collect();
    heights.sort_unstable();
    let child_gap = heights
        .get(heights.len() / 2)
        .copied()?
        .max(1)
        .saturating_mul(SIDE_BY_SIDE_CHILD_GAP_HEIGHT_MULTIPLIER);
    reconstruct_region_table_with_column_gap(region, page_height, page_number, allow_single_column, child_gap).ok()
}

fn normalize_side_by_side_financial_tables(mut left: Table, mut right: Table) -> (Table, Table) {
    if is_wrapped_financial_side_table(&left) && is_wrapped_financial_side_table(&right) {
        normalize_wrapped_financial_side_table(&mut left);
        normalize_wrapped_financial_side_table(&mut right);
    }
    (left, right)
}

fn is_wrapped_financial_side_table(table: &Table) -> bool {
    if table.cells.first().is_some_and(|row| is_explicit_financial_header(row)) {
        return false;
    }
    let Some(rows) = table.cells.get(1..) else {
        return false;
    };
    if table.cells.first().map_or(0, Vec::len) != WRAPPED_FINANCIAL_COLUMNS
        || rows.is_empty()
        || rows.iter().any(|row| row.len() != WRAPPED_FINANCIAL_COLUMNS)
    {
        return false;
    }

    let min_support = rows
        .len()
        .saturating_mul(SIDE_BY_SIDE_MIN_TRACK_ROW_PERCENT)
        .div_ceil(100)
        .max(2);
    let leading_descriptors = (0..WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS)
        .all(|column| rows.iter().filter(|row| is_descriptor_cell(&row[column])).count() >= min_support);
    let trailing_numeric = (WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS..WRAPPED_FINANCIAL_COLUMNS)
        .all(|column| rows.iter().filter(|row| is_numeric_word(&row[column])).count() >= min_support);

    leading_descriptors && trailing_numeric && has_bounded_financial_continuation(rows)
}

fn has_bounded_financial_continuation(rows: &[Vec<String>]) -> bool {
    (0..rows.len()).any(|start| financial_continuation_end(rows, start).is_some())
}

fn normalize_wrapped_financial_side_table(table: &mut Table) {
    let mut rows = Vec::with_capacity(table.cells.len());
    let Some(header) = table.cells.first() else {
        return;
    };
    rows.push(header.clone());

    let mut index = 1;
    while index < table.cells.len() {
        let row = &table.cells[index];
        let Some(end) = financial_continuation_end(&table.cells, index) else {
            rows.push(row.clone());
            index += 1;
            continue;
        };
        let Some(collapsed) = collapse_financial_rows(&table.cells[index..=end]) else {
            rows.extend_from_slice(&table.cells[index..=end]);
            index = end + 1;
            continue;
        };
        rows.push(collapsed);
        index = end + 1;
    }

    table.cells = rows;
    table.markdown = crate::pdf::table_reconstruct::table_to_markdown(&table.cells);
}

fn financial_row_has_numeric_values(row: &[String]) -> bool {
    row.get(WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS..)
        .is_some_and(|values| values.iter().any(|value| is_numeric_word(value)))
}

fn financial_row_has_descriptor(row: &[String]) -> bool {
    row.get(..WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS)
        .is_some_and(|values| values.iter().any(|value| !value.trim().is_empty()))
}

fn financial_row_is_continuation(row: &[String]) -> bool {
    financial_row_has_descriptor(row)
        && row
            .get(WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS..)
            .is_some_and(|values| values.iter().all(|value| value.trim().is_empty()))
}

fn is_explicit_financial_header(row: &[String]) -> bool {
    row.len() == WRAPPED_FINANCIAL_COLUMNS
        && row.iter().all(|cell| !cell.trim().is_empty())
        && row[..WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS]
            .iter()
            .all(|cell| is_descriptor_cell(cell))
        && row[WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS..]
            .iter()
            .all(|cell| is_descriptor_cell(cell))
}

fn financial_continuation_end(rows: &[Vec<String>], start: usize) -> Option<usize> {
    let mut continuation_rows = 0;
    for (index, row) in rows.iter().enumerate().skip(start) {
        if financial_row_is_continuation(row) {
            continuation_rows += 1;
            if continuation_rows > WRAPPED_FINANCIAL_MAX_CONTINUATION_ROWS {
                return None;
            }
            continue;
        }
        return ((WRAPPED_FINANCIAL_MIN_CONTINUATION_ROWS..=WRAPPED_FINANCIAL_MAX_CONTINUATION_ROWS)
            .contains(&continuation_rows)
            && financial_row_has_numeric_values(row))
        .then_some(index);
    }
    None
}

fn collapse_financial_rows(rows: &[Vec<String>]) -> Option<Vec<String>> {
    let terminal = rows.last()?;
    let descriptor = rows
        .iter()
        .flat_map(|row| row.iter().take(WRAPPED_FINANCIAL_DESCRIPTOR_COLUMNS))
        .map(|cell| cell.trim())
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(vec![
        descriptor,
        String::new(),
        terminal.get(2)?.clone(),
        terminal.get(3)?.clone(),
    ])
}

fn side_tables_have_independent_shape(left: &Table, right: &Table) -> bool {
    let Some(left_shape) = side_table_shape(left) else {
        return false;
    };
    let Some(right_shape) = side_table_shape(right) else {
        return false;
    };
    left_shape.columns.abs_diff(right_shape.columns) <= SIDE_BY_SIDE_MAX_TRACK_DELTA
        && left_shape.numeric_tracks.abs_diff(right_shape.numeric_tracks) <= SIDE_BY_SIDE_MAX_TRACK_DELTA
}

fn side_table_shape(table: &Table) -> Option<SideTableShape> {
    let columns = table.cells.first()?.len();
    let rows = table.cells.get(1..)?;
    if rows.is_empty() || rows.iter().any(|row| row.len() != columns) {
        return None;
    }
    let min_support = rows
        .len()
        .saturating_mul(SIDE_BY_SIDE_MIN_TRACK_ROW_PERCENT)
        .div_ceil(100)
        .max(2);
    let descriptor_tracks = (0..columns)
        .filter(|column| rows.iter().filter(|row| is_descriptor_cell(&row[*column])).count() >= min_support)
        .count();
    let numeric_tracks = (0..columns)
        .filter(|column| rows.iter().filter(|row| is_numeric_word(&row[*column])).count() >= min_support)
        .count();
    (descriptor_tracks >= 1 && numeric_tracks >= SIDE_BY_SIDE_MIN_NUMERIC_TRACKS).then_some(SideTableShape {
        columns,
        numeric_tracks,
    })
}

fn is_descriptor_cell(text: &str) -> bool {
    !is_numeric_word(text) && text.chars().filter(|character| character.is_alphabetic()).count() >= 3
}

fn split_side_by_side_region(
    region: &[crate::pdf::table_reconstruct::HocrWord],
) -> Option<(
    Vec<crate::pdf::table_reconstruct::HocrWord>,
    Vec<crate::pdf::table_reconstruct::HocrWord>,
)> {
    if region.len() < SIDE_BY_SIDE_MIN_WORDS_PER_SIDE.saturating_mul(2) {
        return None;
    }

    let (region_left, region_right) = region_horizontal_bounds(region)?;
    let region_width = region_right.saturating_sub(region_left);
    if region_width == 0 {
        return None;
    }
    let seam = central_region_seam(region, region_left, region_width)?;
    let (left, right) = partition_region_at_seam(region, seam)?;
    if left.len() < SIDE_BY_SIDE_MIN_WORDS_PER_SIDE || right.len() < SIDE_BY_SIDE_MIN_WORDS_PER_SIDE {
        return None;
    }
    balanced_region_sides(&left, &right, region_left, region_right, region_width).then_some((left, right))
}

fn region_horizontal_bounds(region: &[crate::pdf::table_reconstruct::HocrWord]) -> Option<(u32, u32)> {
    Some((
        region.iter().map(|word| word.left).min()?,
        region.iter().map(|word| word.left.saturating_add(word.width)).max()?,
    ))
}

fn central_region_seam(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    region_left: u32,
    region_width: u32,
) -> Option<u64> {
    let region_center = u64::from(region_left) + u64::from(region_width) / 2;
    let tolerance = u64::from(region_width).saturating_mul(SIDE_BY_SIDE_CENTER_TOLERANCE_PERCENT) / 100;
    let min_gutter = u64::from(median_word_height(region)).saturating_mul(SIDE_BY_SIDE_MIN_GUTTER_HEIGHT_PERCENT) / 100;
    merged_horizontal_intervals(region)
        .windows(2)
        .filter_map(|pair| {
            let gap = u64::from(pair[1].0.saturating_sub(pair[0].1));
            let center = (u64::from(pair[0].1) + u64::from(pair[1].0)) / 2;
            (gap >= min_gutter && center.abs_diff(region_center) <= tolerance).then_some((gap, center))
        })
        .max_by_key(|(gap, _)| *gap)
        .map(|(_, center)| center)
}

fn merged_horizontal_intervals(region: &[crate::pdf::table_reconstruct::HocrWord]) -> Vec<(u32, u32)> {
    let mut intervals: Vec<_> = region
        .iter()
        .map(|word| (word.left, word.left.saturating_add(word.width)))
        .collect();
    intervals.sort_unstable_by_key(|interval| interval.0);
    let mut merged = Vec::<(u32, u32)>::new();
    for interval in intervals {
        match merged.last_mut() {
            Some(previous) if interval.0 <= previous.1 => previous.1 = previous.1.max(interval.1),
            _ => merged.push(interval),
        }
    }
    merged
}

fn median_word_height(region: &[crate::pdf::table_reconstruct::HocrWord]) -> u32 {
    let mut heights: Vec<u32> = region.iter().map(|word| word.height).collect();
    heights.sort_unstable();
    heights.get(heights.len() / 2).copied().unwrap_or(1).max(1)
}

fn partition_region_at_seam(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    seam: u64,
) -> Option<(
    Vec<crate::pdf::table_reconstruct::HocrWord>,
    Vec<crate::pdf::table_reconstruct::HocrWord>,
)> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for word in region {
        if u64::from(word.left.saturating_add(word.width)) <= seam {
            left.push(word.clone());
        } else if u64::from(word.left) >= seam {
            right.push(word.clone());
        } else {
            return None;
        }
    }
    Some((left, right))
}

fn balanced_region_sides(
    left: &[crate::pdf::table_reconstruct::HocrWord],
    right: &[crate::pdf::table_reconstruct::HocrWord],
    region_left: u32,
    region_right: u32,
    region_width: u32,
) -> bool {
    let left_right = left.iter().map(|word| word.left.saturating_add(word.width)).max();
    let right_left = right.iter().map(|word| word.left).min();
    let (Some(left_right), Some(right_left)) = (left_right, right_left) else {
        return false;
    };
    let left_width = u64::from(left_right.saturating_sub(region_left));
    let right_width = u64::from(region_right.saturating_sub(right_left));
    let minimum = u64::from(region_width).saturating_mul(SIDE_BY_SIDE_MIN_WIDTH_PERCENT) / 100;
    left_width >= minimum && right_width >= minimum
}

/// Cluster words on a single page into vertically-contiguous regions.
///
/// Splits the page on row gaps that are abnormally large compared to the
/// median word height. Each region is a slab of words that may form a table;
/// the caller validates with `reconstruct_table` + `post_process_table`.
fn cluster_words_into_vertical_regions(
    words: &[crate::pdf::table_reconstruct::HocrWord],
) -> Vec<Vec<crate::pdf::table_reconstruct::HocrWord>> {
    if words.len() < 4 {
        return Vec::new();
    }

    let mut heights: Vec<u32> = words.iter().map(|w| w.height).collect();
    heights.sort_unstable();
    let median_height = heights[heights.len() / 2].max(1);
    let row_tolerance = (median_height / 2).max(3);
    let row_gap_split = (median_height as f32 * 1.8) as u32;

    let mut sorted = words.to_vec();
    sorted.sort_by_key(|w| w.top + w.height / 2);

    let mut regions: Vec<Vec<crate::pdf::table_reconstruct::HocrWord>> = Vec::new();
    let mut current: Vec<crate::pdf::table_reconstruct::HocrWord> = Vec::new();
    let mut last_row_yc: Option<u32> = None;

    let mut idx = 0;
    while idx < sorted.len() {
        let row_yc = sorted[idx].top + sorted[idx].height / 2;
        let mut end = idx + 1;
        while end < sorted.len() {
            let yc = sorted[end].top + sorted[end].height / 2;
            if yc.abs_diff(row_yc) <= row_tolerance {
                end += 1;
            } else {
                break;
            }
        }

        if let Some(prev_yc) = last_row_yc
            && row_yc > prev_yc
            && row_yc - prev_yc > row_gap_split
            && !current.is_empty()
        {
            regions.push(std::mem::take(&mut current));
        }
        current.extend(sorted[idx..end].iter().cloned());
        last_row_yc = Some(row_yc);
        idx = end;
    }
    if !current.is_empty() {
        regions.push(current);
    }

    attach_aligned_numeric_headers(&mut regions, median_height, row_tolerance);

    regions.retain(|r| {
        if r.len() < 4 {
            return false;
        }
        let mut row_ycs: Vec<u32> = r.iter().map(|w| w.top + w.height / 2).collect();
        row_ycs.sort_unstable();
        row_ycs.dedup_by(|a, b| a.abs_diff(*b) <= row_tolerance);
        if row_ycs.len() < 3 {
            return false;
        }
        let mut xs: Vec<u32> = r.iter().map(|w| w.left).collect();
        xs.sort_unstable();
        xs.dedup_by(|a, b| a.abs_diff(*b) <= 8);
        xs.len() >= 2
    });

    regions
}

fn attach_aligned_numeric_headers(
    regions: &mut Vec<Vec<crate::pdf::table_reconstruct::HocrWord>>,
    median_height: u32,
    row_tolerance: u32,
) {
    let mut index = 1;
    while index < regions.len() {
        if is_aligned_numeric_header(&regions[index - 1], &regions[index], median_height, row_tolerance) {
            let data = regions.remove(index);
            regions[index - 1].extend(data);
            tracing::trace!(
                index = index - 1,
                "attached aligned header to compact numeric table region"
            );
        } else {
            index += 1;
        }
    }
}

fn is_aligned_numeric_header(
    header: &[crate::pdf::table_reconstruct::HocrWord],
    data: &[crate::pdf::table_reconstruct::HocrWord],
    median_height: u32,
    row_tolerance: u32,
) -> bool {
    let data_rows = numeric_rows(data, row_tolerance);
    let Some(recurring_rows) = longest_recurring_numeric_run(&data_rows) else {
        return false;
    };
    let tracks = recurring_numeric_track_centers(recurring_rows, median_height);
    let header_rows = numeric_rows(header, row_tolerance).len();
    if tracks.len() < DENSE_NUMERIC_MIN_RECURRING_TRACKS || !(1..=NUMERIC_HEADER_MAX_ROWS).contains(&header_rows) {
        return false;
    }

    let alpha_words = header
        .iter()
        .filter(|word| word.text.chars().any(char::is_alphabetic))
        .count();
    if alpha_words.saturating_mul(100) < header.len().saturating_mul(NUMERIC_HEADER_MIN_ALPHA_PERCENT) {
        return false;
    }

    let header_bottom = header.iter().map(|word| word.top + word.height).max().unwrap_or(0);
    let data_top = data.iter().map(|word| word.top).min().unwrap_or(u32::MAX);
    if data_top.saturating_sub(header_bottom) > median_height.saturating_mul(2) {
        return false;
    }

    let x_tolerance = median_height.saturating_mul(2).max(12);
    let matched_tracks = tracks
        .iter()
        .filter(|track| {
            header
                .iter()
                .any(|word| (word.left + word.width / 2).abs_diff(**track) <= x_tolerance)
        })
        .count();
    matched_tracks.saturating_mul(100) >= tracks.len().saturating_mul(NUMERIC_HEADER_MIN_TRACK_PERCENT)
}

/// Reconstruct a single region's words into a `Table`, applying the same
/// validation chain the layout-detection path uses (`layout_guided = true`).
fn reconstruct_region_table(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    page_height: f32,
    page_number: u32,
    allow_single_column: bool,
) -> Option<Table> {
    match reconstruct_region_table_with_reason(region, page_height, page_number, allow_single_column) {
        Ok(table) => Some(table),
        Err(reason) => {
            tracing::trace!(
                page = page_number,
                words = region.len(),
                ?reason,
                "heuristic table region rejected"
            );
            None
        }
    }
}

fn reconstruct_region_table_with_reason(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    page_height: f32,
    page_number: u32,
    allow_single_column: bool,
) -> std::result::Result<Table, HeuristicTableRejection> {
    let region_left = region.iter().map(|w| w.left).min().unwrap_or(0);
    let region_right = region.iter().map(|w| w.left + w.width).max().unwrap_or(0);
    let region_width = region_right.saturating_sub(region_left) as f32;
    let col_gap = heuristic_column_gap(region, region_width);
    reconstruct_region_table_with_column_gap(region, page_height, page_number, allow_single_column, col_gap)
}

fn reconstruct_region_table_with_column_gap(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    page_height: f32,
    page_number: u32,
    allow_single_column: bool,
    col_gap: u32,
) -> std::result::Result<Table, HeuristicTableRejection> {
    use crate::pdf::table_reconstruct::{
        is_well_formed_table, looks_like_code_listing, post_process_table, reconstruct_table, table_to_markdown,
    };

    let column_positions = crate::table_core::detect_columns(region, col_gap);
    let mut grid = reconstruct_table(region, col_gap, 0.5);
    repair_split_numeric_track(&mut grid, region, &column_positions);
    if grid.is_empty() || grid[0].is_empty() {
        return Err(HeuristicTableRejection::EmptyGrid);
    }

    tracing::trace!(
        page = page_number,
        col_gap,
        rows = grid.len(),
        cols = grid.first().map_or(0, Vec::len),
        "heuristic table reconstructed grid"
    );

    let mut cleaned =
        post_process_table(grid, true, allow_single_column).ok_or(HeuristicTableRejection::PostProcessing)?;
    repair_heuristic_header_delimiters(&mut cleaned);
    if cleaned.len() <= 1 {
        return Err(HeuristicTableRejection::TooFewRows);
    }

    if looks_like_code_listing(&cleaned) {
        return Err(HeuristicTableRejection::CodeListing);
    }

    if !is_well_formed_table(&cleaned) {
        return Err(HeuristicTableRejection::NotWellFormed);
    }

    let img_left = region.iter().map(|w| w.left as f64).fold(f64::INFINITY, f64::min);
    let img_top = region.iter().map(|w| w.top as f64).fold(f64::INFINITY, f64::min);
    let img_right = region.iter().map(|w| (w.left + w.width) as f64).fold(0.0_f64, f64::max);
    let img_bottom = region.iter().map(|w| (w.top + w.height) as f64).fold(0.0_f64, f64::max);
    let bounding_box = if img_right > img_left && img_bottom > img_top {
        Some(BoundingBox {
            x0: img_left,
            y0: page_height as f64 - img_bottom,
            x1: img_right,
            y1: page_height as f64 - img_top,
        })
    } else {
        None
    };

    let markdown = table_to_markdown(&cleaned);
    if markdown.trim().is_empty() {
        return Err(HeuristicTableRejection::EmptyMarkdown);
    }

    Ok(Table {
        cells: cleaned,
        markdown,
        page_number,
        bounding_box,
        ..Default::default()
    })
}

fn repair_heuristic_header_delimiters(rows: &mut [Vec<String>]) {
    let Some(header) = rows.first_mut() else {
        return;
    };

    for column in 0..header.len().saturating_sub(1) {
        let (left_cells, right_cells) = header.split_at_mut(column + 1);
        let left = &mut left_cells[column];
        let right = &mut right_cells[0];

        let Some((closer, remainder)) = leading_single_closer(right) else {
            continue;
        };
        if unmatched_opener(left) != matching_opener(closer) {
            continue;
        }

        left.push(closer);
        *right = remainder.to_string();
    }
}

fn leading_single_closer(text: &str) -> Option<(char, &str)> {
    let mut chars = text.chars();
    let closer = chars.next()?;
    matching_opener(closer)?;

    let remainder = chars.as_str().trim_start();
    if remainder.is_empty()
        || remainder
            .chars()
            .next()
            .is_some_and(|character| matching_opener(character).is_some())
    {
        return None;
    }

    Some((closer, remainder))
}

fn unmatched_opener(text: &str) -> Option<char> {
    let mut openers = Vec::new();
    for character in text.chars() {
        if matches!(character, '(' | '[' | '{') {
            openers.push(character);
            continue;
        }

        let Some(expected) = matching_opener(character) else {
            continue;
        };
        if openers.pop() != Some(expected) {
            return None;
        }
    }
    openers.last().copied()
}

fn matching_opener(closer: char) -> Option<char> {
    match closer {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

fn heuristic_column_gap(region: &[crate::pdf::table_reconstruct::HocrWord], region_width: f32) -> u32 {
    let adaptive_gap = crate::pdf::structure::regions::tables::compute_adaptive_column_gap(region, region_width);
    if is_dense_numeric_region(region) {
        adaptive_gap.min(DENSE_NUMERIC_COLUMN_GAP_CAP)
    } else {
        adaptive_gap
    }
}

/// Merge one numeric x-track that was split by anchored column clustering.
///
/// `detect_columns` groups against each cluster's first x-position. A header
/// glyph at the edge of a numeric column can therefore anchor one cluster and
/// split values with a few points of normal alignment jitter into a second,
/// nearly coincident track. Restrict the repair to geometrically overlapping
/// tracks whose numeric cells alternate, with header text on exactly one side.
fn repair_split_numeric_track(
    grid: &mut [Vec<String>],
    region: &[crate::pdf::table_reconstruct::HocrWord],
    column_positions: &[u32],
) -> bool {
    if grid.first().map_or(0, Vec::len) != column_positions.len() {
        return false;
    }

    let candidates: Vec<(usize, bool)> = (0..column_positions.len().saturating_sub(1))
        .filter_map(|column| {
            split_numeric_track_candidate(grid, region, column_positions, column)
                .map(|header_on_left| (column, header_on_left))
        })
        .collect();
    let [(column, _header_on_left)] = candidates.as_slice() else {
        return false;
    };

    for row in grid {
        let right = row.remove(column + 1);
        if row[*column].trim().is_empty() {
            row[*column] = right;
        }
    }
    true
}

fn split_numeric_track_candidate(
    grid: &[Vec<String>],
    region: &[crate::pdf::table_reconstruct::HocrWord],
    column_positions: &[u32],
    column: usize,
) -> Option<bool> {
    let mut left_numeric = 0usize;
    let mut right_numeric = 0usize;
    let mut left_header = false;
    let mut right_header = false;
    for (row_index, row) in grid.iter().enumerate() {
        let left = row.get(column)?.trim();
        let right = row.get(column + 1)?.trim();
        if !left.is_empty() && !right.is_empty() {
            return None;
        }
        let (text, on_left) = if !left.is_empty() {
            (left, true)
        } else if !right.is_empty() {
            (right, false)
        } else {
            continue;
        };
        if is_numeric_word(text) {
            if on_left {
                left_numeric += 1;
            } else {
                right_numeric += 1;
            }
        } else if row_index >= NUMERIC_HEADER_MAX_ROWS {
            return None;
        } else if on_left {
            left_header = true;
        } else {
            right_header = true;
        }
    }

    if left_numeric < SPLIT_NUMERIC_TRACK_MIN_ROWS_PER_SIDE
        || right_numeric < SPLIT_NUMERIC_TRACK_MIN_ROWS_PER_SIDE
        || left_numeric + right_numeric < SPLIT_NUMERIC_TRACK_MIN_TOTAL_ROWS
        || left_header == right_header
    {
        return None;
    }

    let separation = column_positions[column].abs_diff(column_positions[column + 1]);
    let mut numeric_widths: Vec<u32> = region
        .iter()
        .filter(|word| is_numeric_word(&word.text))
        .filter_map(|word| {
            let nearest = column_positions
                .iter()
                .enumerate()
                .min_by_key(|(_, position)| position.abs_diff(word.left))?
                .0;
            matches!(nearest, current if current == column || current == column + 1).then_some(word.width)
        })
        .collect();
    numeric_widths.sort_unstable();
    let median_width = *numeric_widths.get(numeric_widths.len() / 2)?;
    (separation.saturating_mul(2) < median_width).then_some(left_header)
}

fn is_dense_numeric_region(region: &[crate::pdf::table_reconstruct::HocrWord]) -> bool {
    if region.is_empty() {
        return false;
    }

    let median_height = {
        let mut heights: Vec<u32> = region.iter().map(|word| word.height).collect();
        heights.sort_unstable();
        heights[heights.len() / 2].max(1)
    };
    let row_tolerance = (median_height / 2).max(3);
    let rows = numeric_rows(region, row_tolerance);
    let Some(recurring_rows) = longest_recurring_numeric_run(&rows) else {
        return false;
    };

    recurring_numeric_x_tracks(recurring_rows, median_height) >= DENSE_NUMERIC_MIN_RECURRING_TRACKS
}

fn numeric_rows(
    region: &[crate::pdf::table_reconstruct::HocrWord],
    row_tolerance: u32,
) -> Vec<Vec<&crate::pdf::table_reconstruct::HocrWord>> {
    let mut sorted: Vec<_> = region.iter().collect();
    sorted.sort_by_key(|word| word.top + word.height / 2);
    let mut rows: Vec<Vec<&crate::pdf::table_reconstruct::HocrWord>> = Vec::new();
    for word in sorted {
        let center = word.top + word.height / 2;
        match rows.last_mut() {
            Some(row) if (row[0].top + row[0].height / 2).abs_diff(center) <= row_tolerance => row.push(word),
            _ => rows.push(vec![word]),
        }
    }
    rows
}

fn longest_recurring_numeric_run<'a>(
    rows: &'a [Vec<&'a crate::pdf::table_reconstruct::HocrWord>],
) -> Option<&'a [Vec<&'a crate::pdf::table_reconstruct::HocrWord>]> {
    let mut best = 0..0;
    let mut start = 0;
    for (index, row) in rows.iter().enumerate() {
        if is_numeric_row(row) {
            continue;
        }
        if index - start > best.len() {
            best = start..index;
        }
        start = index + 1;
    }
    if rows.len() - start > best.len() {
        best = start..rows.len();
    }
    (best.len() >= DENSE_NUMERIC_MIN_RECURRING_ROWS).then(|| &rows[best])
}

fn is_numeric_row(row: &[&crate::pdf::table_reconstruct::HocrWord]) -> bool {
    let numeric_words = row.iter().filter(|word| is_numeric_word(&word.text)).count();
    numeric_words >= DENSE_NUMERIC_MIN_WORDS_PER_ROW
        && numeric_words.saturating_mul(100) >= row.len().saturating_mul(DENSE_NUMERIC_MIN_ROW_WORD_PERCENT)
}

fn recurring_numeric_x_tracks(rows: &[Vec<&crate::pdf::table_reconstruct::HocrWord>], median_height: u32) -> usize {
    recurring_numeric_track_centers(rows, median_height).len()
}

fn recurring_numeric_track_centers(
    rows: &[Vec<&crate::pdf::table_reconstruct::HocrWord>],
    median_height: u32,
) -> Vec<u32> {
    let x_tolerance = median_height.saturating_mul(2).max(12);
    let min_row_support = rows
        .len()
        .saturating_mul(DENSE_NUMERIC_MIN_TRACK_ROW_PERCENT)
        .div_ceil(100);
    let mut candidates: Vec<u32> = rows
        .iter()
        .flatten()
        .filter(|word| is_numeric_word(&word.text))
        .map(|word| word.left + word.width / 2)
        .collect();
    candidates.sort_unstable();
    candidates.dedup_by(|left, right| left.abs_diff(*right) <= x_tolerance);
    candidates
        .into_iter()
        .filter(|candidate| {
            rows.iter()
                .filter(|row| {
                    row.iter().any(|word| {
                        is_numeric_word(&word.text) && (word.left + word.width / 2).abs_diff(*candidate) <= x_tolerance
                    })
                })
                .count()
                >= min_row_support
        })
        .collect()
}

fn is_numeric_word(text: &str) -> bool {
    let digit_count = text.chars().filter(char::is_ascii_digit).count();
    if digit_count == 0 {
        return false;
    }
    let alphanumeric_count = text.chars().filter(|c| c.is_alphanumeric()).count();
    digit_count.saturating_mul(2) >= alphanumeric_count
}

/// Reconstruct cell text from span positions in reading order.
///
/// PDF coordinates place y=0 at the bottom of the page, so larger Y values are
/// higher on the page (visually earlier in reading order). Within a row, X
/// increases left-to-right. We sort by Y descending (top-to-bottom) then X
/// ascending (left-to-right) to recover natural reading order regardless of
/// the order in which pdf_oxide yields spans.
///
/// Embedded newlines inside span text are collapsed to spaces to produce
/// clean single-line cell strings.
fn cell_text_in_reading_order(cell: &pdf_oxide::structure::table_extractor::TableCell) -> String {
    if cell.spans.is_empty() {
        return cell.text.trim().replace('\n', " ").to_string();
    }

    let mut sorted: Vec<(f32, f32, &str)> = cell
        .spans
        .iter()
        .map(|span| (span.bbox.y, span.bbox.x, span.text.as_str()))
        .collect();
    sorted.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.total_cmp(&b.1)));

    let joined: String = sorted
        .iter()
        .map(|(_, _, text)| text.trim().replace('\n', " "))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    joined
}

/// Convert a pdf_oxide `ExtractedTable` to xberg's cell grid and markdown.
///
/// Maps rows/cells from the native table structure to a 2D `Vec<Vec<String>>`
/// grid and builds a markdown representation with proper header separators.
///
/// Cell text is reconstructed from span positions in reading order
/// (Y descending, X ascending) when span data is available.
fn convert_extracted_table(table: &pdf_oxide::structure::table_extractor::Table) -> (Vec<Vec<String>>, String) {
    let mut cells: Vec<Vec<String>> = Vec::with_capacity(table.rows.len());
    let mut markdown = String::new();
    let mut found_header = false;

    for (row_idx, row) in table.rows.iter().enumerate() {
        let row_cells: Vec<String> = row.cells.iter().map(cell_text_in_reading_order).collect();

        markdown.push('|');
        for cell in &row_cells {
            markdown.push(' ');
            markdown.push_str(cell);
            markdown.push_str(" |");
        }
        markdown.push('\n');

        if (row.is_header || row_idx == 0) && !found_header {
            found_header = true;
            markdown.push('|');
            for _ in &row_cells {
                markdown.push_str(" --- |");
            }
            markdown.push('\n');
        }

        cells.push(row_cells);
    }

    (cells, markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_extracted_table_basic() {
        use pdf_oxide::structure::table_extractor::{Table as ExtractedTable, TableCell, TableRow};

        let table = ExtractedTable {
            rows: vec![
                TableRow {
                    cells: vec![
                        TableCell {
                            text: "Name".to_string(),
                            colspan: 1,
                            rowspan: 1,
                            mcids: vec![],
                            spans: vec![],
                            bbox: None,
                            is_header: true,
                        },
                        TableCell {
                            text: "Age".to_string(),
                            colspan: 1,
                            rowspan: 1,
                            mcids: vec![],
                            spans: vec![],
                            bbox: None,
                            is_header: true,
                        },
                    ],
                    is_header: true,
                },
                TableRow {
                    cells: vec![
                        TableCell {
                            text: "Alice".to_string(),
                            colspan: 1,
                            rowspan: 1,
                            mcids: vec![],
                            spans: vec![],
                            bbox: None,
                            is_header: false,
                        },
                        TableCell {
                            text: "30".to_string(),
                            colspan: 1,
                            rowspan: 1,
                            mcids: vec![],
                            spans: vec![],
                            bbox: None,
                            is_header: false,
                        },
                    ],
                    is_header: false,
                },
            ],
            has_header: true,
            col_count: 2,
            bbox: None,
        };

        let (cells, markdown) = convert_extracted_table(&table);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0], vec!["Name", "Age"]);
        assert_eq!(cells[1], vec!["Alice", "30"]);
        assert!(markdown.contains("| Name | Age |"));
        assert!(markdown.contains("| --- | --- |"));
        assert!(markdown.contains("| Alice | 30 |"));
    }

    #[test]
    fn test_convert_extracted_table_no_header() {
        use pdf_oxide::structure::table_extractor::{Table as ExtractedTable, TableCell, TableRow};

        let table = ExtractedTable {
            rows: vec![
                TableRow {
                    cells: vec![TableCell {
                        text: "A".to_string(),
                        colspan: 1,
                        rowspan: 1,
                        mcids: vec![],
                        spans: vec![],
                        bbox: None,
                        is_header: false,
                    }],
                    is_header: false,
                },
                TableRow {
                    cells: vec![TableCell {
                        text: "B".to_string(),
                        colspan: 1,
                        rowspan: 1,
                        mcids: vec![],
                        spans: vec![],
                        bbox: None,
                        is_header: false,
                    }],
                    is_header: false,
                },
            ],
            has_header: false,
            col_count: 1,
            bbox: None,
        };

        let (cells, markdown) = convert_extracted_table(&table);
        assert_eq!(cells.len(), 2);
        assert!(markdown.contains("| --- |"));
    }

    #[test]
    fn test_convert_extracted_table_empty() {
        use pdf_oxide::structure::table_extractor::Table as ExtractedTable;

        let table = ExtractedTable {
            rows: vec![],
            has_header: false,
            col_count: 0,
            bbox: None,
        };

        let (cells, markdown) = convert_extracted_table(&table);
        assert!(cells.is_empty());
        assert!(markdown.is_empty());
    }

    /// Build a synthetic TextSpan for position-order tests.
    ///
    /// `x` and `y` are the span's PDF-coordinate origin (y=0 at bottom of page).
    fn make_span(text: &str, x: f32, y: f32) -> pdf_oxide::layout::TextSpan {
        pdf_oxide::layout::TextSpan {
            text: text.to_string(),
            bbox: pdf_oxide::geometry::Rect {
                x,
                y,
                width: 50.0,
                height: 10.0,
            },
            font_name: "Helvetica".to_string(),
            font_size: 10.0,
            font_weight: pdf_oxide::layout::FontWeight::Normal,
            is_italic: false,
            is_monospace: false,
            color: pdf_oxide::layout::Color::default(),
            mcid: None,
            sequence: 0,
            split_boundary_before: false,
            offset_semantic: false,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scaling: 1.0,
            primary_detected: false,
            artifact_type: None,
            char_widths: vec![],
            heading_level: None,
            rotation_degrees: 0.0,
            ..pdf_oxide::layout::TextSpan::default()
        }
    }

    /// Spans delivered in reverse Y order (bottom span first, top span last).
    /// Reading order must place the top span (higher Y in PDF coords) first.
    #[test]
    fn test_cell_text_in_reading_order_sorts_by_y_descending() {
        use pdf_oxide::structure::table_extractor::TableCell;

        let cell = TableCell {
            text: "wrong order".to_string(),
            colspan: 1,
            rowspan: 1,
            mcids: vec![],
            spans: vec![make_span("second", 10.0, 100.0), make_span("first", 10.0, 200.0)],
            bbox: None,
            is_header: false,
        };

        let text = cell_text_in_reading_order(&cell);
        assert_eq!(
            text, "first second",
            "span with higher Y (top of page) must come before span with lower Y; got: {text:?}"
        );
    }

    /// Within the same Y row, spans must be ordered left-to-right (X ascending).
    #[test]
    fn test_cell_text_in_reading_order_sorts_same_y_by_x_ascending() {
        use pdf_oxide::structure::table_extractor::TableCell;

        let cell = TableCell {
            text: "wrong order".to_string(),
            colspan: 1,
            rowspan: 1,
            mcids: vec![],
            spans: vec![make_span("right", 200.0, 150.0), make_span("left", 10.0, 150.0)],
            bbox: None,
            is_header: false,
        };

        let text = cell_text_in_reading_order(&cell);
        assert_eq!(
            text, "left right",
            "same-row spans must be ordered left-to-right (X ascending); got: {text:?}"
        );
    }

    /// Build a synthetic `HocrWord` for the clustering tests.
    fn make_word(text: &str, left: u32, top: u32, width: u32) -> crate::pdf::table_reconstruct::HocrWord {
        crate::pdf::table_reconstruct::HocrWord {
            text: text.to_string(),
            left,
            top,
            width,
            height: 10,
            confidence: 95.0,
        }
    }

    fn extend_grid_words(words: &mut Vec<crate::pdf::table_reconstruct::HocrWord>, x_positions: &[u32]) {
        for left in x_positions {
            words.push(make_word("Heading", *left, 100, 30));
        }
        for row in 0..6 {
            for (column, left) in x_positions.iter().enumerate() {
                words.push(make_word(&format!("{}.{column}", row + 1), *left, 130 + row * 12, 30));
            }
        }
    }

    fn extend_financial_table_words(
        words: &mut Vec<crate::pdf::table_reconstruct::HocrWord>,
        x_positions: &[u32],
        descriptor_prefix: &str,
    ) {
        for (column, left) in x_positions.iter().enumerate() {
            let heading = if column == 0 { "Security" } else { "Value" };
            words.push(make_word(heading, *left, 100, 30));
        }
        for row in 0..8 {
            words.push(make_word(
                &format!("{descriptor_prefix} bond {row}"),
                x_positions[0],
                130 + row * 12,
                60,
            ));
            for (column, left) in x_positions.iter().enumerate().skip(1) {
                words.push(make_word(
                    &format!("{},{}", row + 1, column * 113),
                    *left,
                    130 + row * 12,
                    30,
                ));
            }
        }
    }

    #[test]
    fn splits_independently_valid_side_by_side_tables() {
        let mut words = Vec::new();
        extend_financial_table_words(&mut words, &[20, 140, 220], "Cayman");
        extend_financial_table_words(&mut words, &[320, 400, 480, 560], "Ireland");

        let (left, right) = split_side_by_side_region(&words).expect("central seam should be credible");
        assert!(
            reconstruct_region_table(&left, 792.0, 1, false).is_some(),
            "left child must independently pass the existing validation chain"
        );
        assert!(
            reconstruct_region_table(&right, 792.0, 1, false).is_some(),
            "right child must independently pass the existing validation chain"
        );

        let tables = reconstruct_region_tables(&words, 792.0, 1, false);
        assert_eq!(
            tables.len(),
            2,
            "valid side-by-side children should replace the fused parent"
        );
        assert_eq!(tables[0].cells[0].len(), 3);
        assert_eq!(tables[1].cells[0].len(), 4);
    }

    fn wrapped_financial_side(prefix: &str) -> Table {
        let cells = vec![
            vec![
                format!("{prefix} Alpha"),
                "Series 2024-A".to_string(),
                String::new(),
                String::new(),
            ],
            vec![
                "Class A1R".to_string(),
                "three-month SOFR".to_string(),
                String::new(),
                String::new(),
            ],
            vec![
                "5.87%".to_string(),
                "04/15/37".to_string(),
                "5,718".to_string(),
                "5,742,049".to_string(),
            ],
            vec![
                format!("{prefix} Beta"),
                "Series 2025-B".to_string(),
                String::new(),
                String::new(),
            ],
            vec![
                "Class BR".to_string(),
                "three-month SOFR".to_string(),
                String::new(),
                String::new(),
            ],
            vec![
                "6.08%".to_string(),
                "01/15/34".to_string(),
                "500".to_string(),
                "500,639".to_string(),
            ],
        ];
        Table {
            markdown: crate::pdf::table_reconstruct::table_to_markdown(&cells),
            cells,
            ..Default::default()
        }
    }

    #[test]
    fn normalizes_wrapped_side_by_side_financial_tables() {
        let left = wrapped_financial_side("Cayman");
        let right = wrapped_financial_side("Ireland");

        let (left, right) = normalize_side_by_side_financial_tables(left, right);

        assert_eq!(left.cells.len(), 4);
        assert_eq!(right.cells.len(), 4);
        assert_eq!(
            left.cells[3],
            vec![
                "Cayman Beta Series 2025-B Class BR three-month SOFR 6.08% 01/15/34",
                "",
                "500",
                "500,639"
            ]
        );
        assert!(
            left.cells.iter().all(|row| row.len() == WRAPPED_FINANCIAL_COLUMNS),
            "normalization must preserve a rectangular four-column grid"
        );
        assert!(
            left.markdown
                .lines()
                .filter(|line| line.starts_with('|'))
                .all(|line| line.matches('|').count() == WRAPPED_FINANCIAL_COLUMNS + 1),
            "every markdown row and separator must retain four cells"
        );
        assert!(left.markdown.contains("|  | 500 | 500,639 |"));
    }

    #[test]
    fn preserves_ordinary_four_column_side_by_side_tables() {
        let cells = vec![
            vec!["Security", "Region", "Quantity", "Value"],
            vec!["Alpha", "Cayman", "100", "1,000"],
            vec!["Beta", "Ireland", "200", "2,000"],
            vec!["Gamma", "France", "300", "3,000"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect())
        .collect::<Vec<Vec<String>>>();
        let table = Table {
            markdown: crate::pdf::table_reconstruct::table_to_markdown(&cells),
            cells,
            ..Default::default()
        };

        let (left, right) = normalize_side_by_side_financial_tables(table.clone(), table.clone());

        assert_eq!(left.cells, table.cells);
        assert_eq!(right.cells, table.cells);
        assert_eq!(left.markdown, table.markdown);
        assert_eq!(right.markdown, table.markdown);
    }

    #[test]
    fn preserves_semantic_four_column_financial_tables_with_wrapped_descriptors() {
        let cells = vec![
            vec!["Security", "Instrument", "Par", "Value"],
            vec!["Alpha Holdings", "Senior secured note", "", ""],
            vec!["Series 2024-A", "Floating rate", "", ""],
            vec!["Matures 04/15/37", "USD", "5,718", "5,742,049"],
            vec!["Beta Holdings", "Senior secured note", "", ""],
            vec!["Series 2025-B", "Fixed rate", "", ""],
            vec!["Matures 01/15/34", "EUR", "500", "500,639"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect())
        .collect::<Vec<Vec<String>>>();
        let table = Table {
            markdown: crate::pdf::table_reconstruct::table_to_markdown(&cells),
            cells,
            ..Default::default()
        };

        let (left, right) = normalize_side_by_side_financial_tables(table.clone(), table.clone());

        assert_eq!(left.cells, table.cells);
        assert_eq!(right.cells, table.cells);
        assert_eq!(left.markdown, table.markdown);
        assert_eq!(right.markdown, table.markdown);
    }

    #[test]
    fn does_not_merge_financial_continuation_across_intervening_row() {
        let mut table = wrapped_financial_side("Cayman");
        table.cells.insert(
            1,
            vec![
                "Disclosure".to_string(),
                String::new(),
                "not applicable".to_string(),
                String::new(),
            ],
        );
        table.markdown = crate::pdf::table_reconstruct::table_to_markdown(&table.cells);

        let (left, _) = normalize_side_by_side_financial_tables(table.clone(), table);

        assert_eq!(left.cells[0][0], "Cayman Alpha");
        assert_eq!(
            left.cells[1],
            vec!["Disclosure", "", "not applicable", ""],
            "the intervening row must remain byte-for-byte unchanged"
        );
        assert!(
            !left.cells[0][0].contains("Class A1R"),
            "an intervening non-continuation row must stop coalescing"
        );
    }

    #[test]
    fn preserves_ordinary_five_column_table() {
        let mut words = Vec::new();
        extend_grid_words(&mut words, &[20, 120, 220, 320, 420]);

        assert!(
            split_side_by_side_region(&words).is_none(),
            "occupied central column must prevent a side-by-side split"
        );
    }

    #[test]
    fn preserves_grouped_eight_column_table_without_descriptors() {
        let mut words = Vec::new();
        extend_grid_words(&mut words, &[20, 100, 180, 260]);
        extend_grid_words(&mut words, &[320, 400, 480, 560]);

        let tables = reconstruct_region_tables(&words, 792.0, 1, false);
        assert_eq!(
            tables.len(),
            1,
            "central grouping alone must not split an ordinary table"
        );
        assert_eq!(tables[0].cells[0].len(), 8);
    }

    /// Heuristic table extraction must not panic on an empty / minimal PDF.
    #[test]
    fn test_extract_tables_heuristic_minimal_pdf_no_panic() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/tiny.pdf");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(&path).expect("read tiny.pdf");
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open tiny.pdf");
        let skip = HashSet::new();
        let extraction =
            extract_tables_heuristic(&mut doc, false, &skip).expect("heuristic must not error on minimal PDF");
        assert!(
            extraction.tables.is_empty(),
            "expected no tables on minimal PDF, got: {:?}",
            extraction.tables
        );
    }

    /// On a real text-layer PDF that contains a table, the heuristic should
    /// produce at least one well-formed `Table` (rows≥2, cols≥2) with a
    /// non-empty markdown rendering. Regression test for #897.
    #[test]
    fn test_extract_tables_heuristic_recovers_table_document() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/table_document.pdf");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(&path).expect("read table_document.pdf");
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open table_document.pdf");

        let skip = HashSet::new();
        let page_count = doc.doc.page_count().expect("table fixture page count");
        let extraction = extract_tables_heuristic(&mut doc, false, &skip).expect("heuristic must not error");
        assert_eq!(
            extraction.hierarchy_segments.pages.len(),
            page_count,
            "owned hierarchy segments must preserve one slot per PDF page"
        );
        let tables = extraction.tables;

        if tables.is_empty() {
            eprintln!(
                "extract_tables_heuristic returned 0 tables on table_document.pdf — \
                 fixture may be borderline for the prose filter; verify manually"
            );
            return;
        }

        for t in &tables {
            assert!(t.cells.len() >= 2, "table has fewer than 2 rows: {t:?}");
            assert!(t.cells.iter().any(|r| r.len() >= 2), "no row has 2+ columns: {t:?}");
            assert!(!t.markdown.trim().is_empty(), "markdown is empty: {t:?}");
            assert!(t.page_number >= 1, "page_number must be 1-indexed: {t:?}");
            assert!(
                t.markdown.contains("| --- |") || t.markdown.contains("|---|"),
                "table markdown is missing the header separator row: {:?}",
                t.markdown
            );
            if let Some(bbox) = &t.bounding_box {
                assert!(
                    bbox.y0 < bbox.y1,
                    "bbox y0 must be less than y1 (PDF coords: bottom < top): {bbox:?}"
                );
                assert!(bbox.x0 < bbox.x1, "bbox x0 must be less than x1: {bbox:?}");
            }
        }
    }

    #[test]
    fn test_extract_tables_heuristic_repairs_embedded_fixture_numeric_track() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/pdf/embedded_images_tables.pdf");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(&path).expect("read embedded table fixture");
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open embedded table fixture");

        let tables = extract_tables_heuristic(&mut doc, false, &HashSet::new())
            .expect("heuristic extraction")
            .tables;
        let table = tables
            .iter()
            .find(|table| table.cells[0].iter().any(|cell| cell.contains("Inhibitor")))
            .expect("polarization table");

        assert_eq!(table.cells.len(), 7);
        assert!(table.cells.iter().all(|row| row.len() == 7), "{:?}", table.cells);
        assert!(
            table.cells[0]
                .windows(2)
                .any(|cells| cells == ["icorr (A/cm2)", "Polarization resistance ( Ω )"]),
            "{:?}",
            table.cells[0]
        );
        assert!(
            table
                .markdown
                .contains("| icorr (A/cm2) | Polarization resistance ( Ω ) |"),
            "{}",
            table.markdown
        );
        assert!(
            table.cells[1..]
                .iter()
                .all(|row| row.iter().all(|cell| !cell.trim().is_empty()))
        );
    }

    /// `skip_pages` (1-indexed) suppresses the heuristic on the listed pages.
    /// This is the per-page composition contract: native finds page N, we
    /// pass `{N}` to the heuristic and it must not emit any tables for page N.
    #[test]
    fn test_extract_tables_heuristic_skip_pages_honored() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/table_document.pdf");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::read(&path).expect("read table_document.pdf");
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open table_document.pdf");

        let baseline = extract_tables_heuristic(&mut doc, false, &HashSet::new())
            .expect("baseline heuristic")
            .tables;
        if baseline.is_empty() {
            return;
        }
        let pages_baseline_touched: HashSet<u32> = baseline.iter().map(|t| t.page_number).collect();

        let skip = pages_baseline_touched.clone();
        let suppressed = extract_tables_heuristic(&mut doc, false, &skip)
            .expect("skip-pages heuristic")
            .tables;

        for t in &suppressed {
            assert!(
                !skip.contains(&t.page_number),
                "page {} appeared in skip set but heuristic still emitted a table: {:?}",
                t.page_number,
                t
            );
        }
    }

    /// Vertical-gap clustering: words all within one band become one region.
    #[test]
    fn test_cluster_words_single_region_not_split() {
        let words = vec![
            make_word("a1", 100, 100, 20),
            make_word("a2", 200, 100, 20),
            make_word("b1", 100, 112, 20),
            make_word("b2", 200, 112, 20),
            make_word("c1", 100, 124, 20),
            make_word("c2", 200, 124, 20),
        ];
        let regions = cluster_words_into_vertical_regions(&words);
        assert_eq!(regions.len(), 1, "expected one region, got {regions:?}");
        assert_eq!(regions[0].len(), 6);
    }

    /// Vertical-gap clustering: words separated by a >1.8× median-height gap
    /// must split into two regions. Both tables sized to survive the retain
    /// filter (≥3 distinct rows, ≥2 distinct x-positions, ≥4 words).
    #[test]
    fn test_cluster_words_two_tables_separated_by_large_gap() {
        let words = vec![
            make_word("a1", 100, 100, 20),
            make_word("a2", 200, 100, 20),
            make_word("b1", 100, 112, 20),
            make_word("b2", 200, 112, 20),
            make_word("c1", 100, 124, 20),
            make_word("c2", 200, 124, 20),
            make_word("d1", 100, 300, 20),
            make_word("d2", 200, 300, 20),
            make_word("e1", 100, 312, 20),
            make_word("e2", 200, 312, 20),
            make_word("f1", 100, 324, 20),
            make_word("f2", 200, 324, 20),
        ];
        let regions = cluster_words_into_vertical_regions(&words);
        assert_eq!(regions.len(), 2, "expected two regions, got {regions:?}");
        for r in &regions {
            assert_eq!(r.len(), 6, "each region should have 6 words: {r:?}");
        }
        let first_top = regions[0].iter().map(|w| w.top).min().unwrap();
        let second_top = regions[1].iter().map(|w| w.top).min().unwrap();
        assert!(first_top < second_top, "regions should be ordered top-to-bottom");
    }

    /// Clustering must reject single-column (only one distinct x-position)
    /// runs — those are lists, not tables.
    #[test]
    fn test_cluster_words_rejects_single_column_run() {
        let words = vec![
            make_word("item1", 100, 100, 40),
            make_word("item2", 100, 112, 40),
            make_word("item3", 100, 124, 40),
            make_word("item4", 100, 136, 40),
        ];
        let regions = cluster_words_into_vertical_regions(&words);
        assert!(
            regions.is_empty(),
            "single-column word run must not survive clustering, got {regions:?}"
        );
    }

    /// Clustering must reject runs with fewer than 3 distinct rows — too small
    /// to be a real table.
    #[test]
    fn test_cluster_words_rejects_two_row_run() {
        let words = vec![
            make_word("a", 100, 100, 20),
            make_word("b", 200, 100, 20),
            make_word("c", 100, 112, 20),
            make_word("d", 200, 112, 20),
        ];
        let regions = cluster_words_into_vertical_regions(&words);
        assert!(
            regions.is_empty(),
            "two-row run must not survive clustering, got {regions:?}"
        );
    }

    /// Clustering returns an empty vec when fewer than 4 input words are given,
    /// since no realistic table can be reconstructed from that.
    #[test]
    fn test_cluster_words_below_minimum_returns_empty() {
        let words = vec![
            make_word("a", 0, 0, 10),
            make_word("b", 0, 12, 10),
            make_word("c", 0, 24, 10),
        ];
        let regions = cluster_words_into_vertical_regions(&words);
        assert!(regions.is_empty());
    }

    #[test]
    fn dense_numeric_region_caps_adaptive_column_gap() {
        let mut words = Vec::new();
        for row in 0..7 {
            for col in 0..7 {
                words.push(make_word("1.000", 20 + col * 120, 100 + row as u32 * 12, 20));
            }
        }

        let adaptive = crate::pdf::structure::regions::tables::compute_adaptive_column_gap(&words, 900.0);
        assert!(adaptive > DENSE_NUMERIC_COLUMN_GAP_CAP);
        assert_eq!(heuristic_column_gap(&words, 900.0), DENSE_NUMERIC_COLUMN_GAP_CAP);
    }

    #[test]
    fn aligned_multiline_header_attaches_to_compact_numeric_table() {
        let mut header = Vec::new();
        for col in 0..7 {
            header.push(make_word("Heading", 20 + col * 120, 100, 50));
            header.push(make_word("unit", 20 + col * 120, 112, 30));
        }
        let mut data = Vec::new();
        for row in 0..6 {
            for col in 0..7 {
                data.push(make_word("1.000", 20 + col * 120, 130 + row * 12, 30));
            }
        }
        let expected_words = header.len() + data.len();
        let mut regions = vec![header, data];

        attach_aligned_numeric_headers(&mut regions, 10, 5);

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].len(), expected_words);
    }

    #[test]
    fn compact_numeric_table_reconstructs_end_to_end() {
        let mut words = Vec::new();
        for col in 0..7 {
            words.push(make_word("Heading", 20 + col * 100, 100, 30));
        }
        for row in 0..6 {
            for col in 0..7 {
                words.push(make_word("1.000", 20 + col * 100, 130 + row * 12, 30));
            }
        }

        let regions = cluster_words_into_vertical_regions(&words);
        assert_eq!(
            regions.len(),
            1,
            "header and compact numeric body should form one candidate"
        );
        let table = reconstruct_region_table(&regions[0], 792.0, 1, false).expect("compact table should reconstruct");

        assert_eq!(table.cells.len(), 7);
        assert!(table.cells.iter().all(|row| row.len() == 7));
        assert!(!table.markdown.trim().is_empty());
    }

    fn alternating_numeric_tracks(
        header: [&str; 2],
        word_width: u32,
    ) -> (Vec<Vec<String>>, Vec<crate::pdf::table_reconstruct::HocrWord>) {
        let mut grid = vec![vec!["ID".to_string(), header[0].to_string(), header[1].to_string()]];
        let mut words = Vec::new();
        for row in 0..6 {
            let mut cells = vec![(row + 1).to_string(), String::new(), String::new()];
            let column = if row % 3 == 0 { 2 } else { 1 };
            cells[column] = format!("{}.{row}", row + 1);
            words.push(make_word(
                &cells[column],
                if column == 1 { 100 } else { 104 },
                120 + row * 12,
                word_width,
            ));
            grid.push(cells);
        }
        (grid, words)
    }

    #[test]
    fn repairs_overlapping_mutually_exclusive_numeric_tracks() {
        let (mut grid, words) = alternating_numeric_tracks(["Polarization resistance", ""], 20);

        assert!(repair_split_numeric_track(&mut grid, &words, &[20, 100, 104]));
        assert!(grid.iter().all(|row| row.len() == 2));
        assert_eq!(grid[0][1], "Polarization resistance");
        assert!(grid[1..].iter().all(|row| !row[1].is_empty()));
    }

    #[test]
    fn preserves_named_adjacent_sparse_numeric_columns() {
        let (mut grid, words) = alternating_numeric_tracks(["Debit", "Credit"], 20);

        assert!(!repair_split_numeric_track(&mut grid, &words, &[20, 100, 104]));
        assert!(grid.iter().all(|row| row.len() == 3));
    }

    #[test]
    fn preserves_unnamed_adjacent_sparse_numeric_columns() {
        let (mut grid, words) = alternating_numeric_tracks(["", ""], 20);

        assert!(!repair_split_numeric_track(&mut grid, &words, &[20, 100, 104]));
        assert!(grid.iter().all(|row| row.len() == 3));
    }

    #[test]
    fn preserves_geometrically_distinct_alternating_numeric_tracks() {
        let (mut grid, words) = alternating_numeric_tracks(["Combined amount", ""], 4);

        assert!(!repair_split_numeric_track(&mut grid, &words, &[20, 100, 104]));
        assert!(grid.iter().all(|row| row.len() == 3));
    }

    #[test]
    fn preserves_alternating_numeric_tracks_with_late_nonnumeric_cell() {
        let (mut grid, words) = alternating_numeric_tracks(["", ""], 20);
        grid.push(vec!["7".to_string(), "N/A footnote".to_string(), String::new()]);

        assert!(!repair_split_numeric_track(&mut grid, &words, &[20, 100, 104]));
        assert!(grid.iter().all(|row| row.len() == 3));
    }

    #[test]
    fn unaligned_prose_does_not_attach_to_compact_numeric_table() {
        let header = vec![
            make_word("This", 500, 100, 30),
            make_word("paragraph", 550, 100, 60),
            make_word("continues", 620, 112, 50),
        ];
        let mut data = Vec::new();
        for row in 0..6 {
            for col in 0..7 {
                data.push(make_word("1.000", 20 + col * 80, 130 + row * 12, 30));
            }
        }
        let mut regions = vec![header, data];

        attach_aligned_numeric_headers(&mut regions, 10, 5);

        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn alphabetic_region_keeps_adaptive_column_gap() {
        let mut words = Vec::new();
        for row in 0..7 {
            for col in 0..8 {
                words.push(make_word("value", 20 + col * 120, 100 + row as u32 * 12, 20));
            }
        }

        let adaptive = crate::pdf::structure::regions::tables::compute_adaptive_column_gap(&words, 900.0);
        assert_eq!(heuristic_column_gap(&words, 900.0), adaptive);
    }

    #[test]
    fn multi_column_prose_does_not_cap_adaptive_column_gap() {
        let mut words = Vec::new();
        for row in 0..8 {
            for col in 0..3 {
                words.push(make_word(
                    if row % 3 == 0 { "2024" } else { "paragraph" },
                    20 + col * 280,
                    100 + row * 12,
                    100,
                ));
            }
        }

        let adaptive = crate::pdf::structure::regions::tables::compute_adaptive_column_gap(&words, 800.0);
        assert_eq!(heuristic_column_gap(&words, 800.0), adaptive);
    }

    #[test]
    fn sparse_numeric_rows_do_not_cap_adaptive_column_gap() {
        let mut words = Vec::new();
        for row in 0..9 {
            let text = if row % 2 == 0 { "1.000" } else { "value" };
            for col in 0..5 {
                words.push(make_word(text, 20 + col * 140, 100 + row * 12, 30));
            }
        }

        let adaptive = crate::pdf::structure::regions::tables::compute_adaptive_column_gap(&words, 700.0);
        assert_eq!(heuristic_column_gap(&words, 700.0), adaptive);
    }

    #[test]
    fn heuristic_empty_region_reports_rejection_reason() {
        let rejection = reconstruct_region_table_with_reason(&[], 792.0, 1, false).unwrap_err();
        assert_eq!(rejection, HeuristicTableRejection::EmptyGrid);
    }

    #[test]
    fn heuristic_code_listing_reports_rejection_reason() {
        let words = vec![
            make_word("fn", 20, 100, 20),
            make_word("main", 100, 100, 30),
            make_word("{", 20, 112, 10),
            make_word("call();", 100, 112, 50),
            make_word("}", 20, 124, 10),
            make_word("// end", 100, 124, 50),
        ];
        let rejection = reconstruct_region_table_with_reason(&words, 792.0, 1, false).unwrap_err();
        assert!(
            matches!(
                rejection,
                HeuristicTableRejection::PostProcessing | HeuristicTableRejection::CodeListing
            ),
            "unexpected rejection: {rejection:?}"
        );
    }

    #[test]
    fn repairs_unmatched_header_delimiter_across_adjacent_cells() {
        let mut rows = vec![
            vec![
                "icorr (A/cm2".to_string(),
                ") Polarization resistance ( Ω )".to_string(),
            ],
            vec!["1.25".to_string(), "320".to_string()],
        ];

        repair_heuristic_header_delimiters(&mut rows);

        assert_eq!(rows[0], ["icorr (A/cm2)", "Polarization resistance ( Ω )"]);
        assert_eq!(rows[1], ["1.25", "320"]);
    }

    #[test]
    fn repairs_each_supported_header_delimiter() {
        for (left, right, expected_left, expected_right) in [
            ("mean (ms", ") latency", "mean (ms)", "latency"),
            ("range [low", "] high", "range [low]", "high"),
            ("mapping {key", "} value", "mapping {key}", "value"),
        ] {
            let mut rows = vec![vec![left.to_string(), right.to_string()]];

            repair_heuristic_header_delimiters(&mut rows);

            assert_eq!(rows[0], [expected_left, expected_right]);
        }
    }

    #[test]
    fn preserves_balanced_mismatched_and_nonleading_header_delimiters() {
        for (left, right) in [
            ("rate (ms)", ") latency"),
            ("range [low", ") latency"),
            ("rate (ms", "latency ) total"),
            ("ordinary header", ") notes"),
            ("rate (ms", ")) latency"),
            ("rate (ms", ") ] latency"),
        ] {
            let mut rows = vec![vec![left.to_string(), right.to_string()]];
            let expected = rows.clone();

            repair_heuristic_header_delimiters(&mut rows);

            assert_eq!(rows, expected, "unexpected repair for {left:?} and {right:?}");
        }
    }

    #[test]
    fn preserves_data_row_delimiter_boundaries() {
        let mut rows = vec![
            vec!["Metric".to_string(), "Value".to_string()],
            vec!["sample (raw".to_string(), ") normalized".to_string()],
        ];
        let expected = rows.clone();

        repair_heuristic_header_delimiters(&mut rows);

        assert_eq!(rows, expected);
    }

    #[test]
    fn respects_nested_header_delimiter_order() {
        let mut matching = vec![vec!["Statistic ([SD".to_string(), "] range".to_string()]];
        repair_heuristic_header_delimiters(&mut matching);
        assert_eq!(matching[0], ["Statistic ([SD]", "range"]);

        let mut mismatched = vec![vec!["Statistic ([SD".to_string(), ") range".to_string()]];
        let expected = mismatched.clone();
        repair_heuristic_header_delimiters(&mut mismatched);
        assert_eq!(mismatched, expected);
    }

    #[test]
    fn test_cell_text_in_reading_order_fallback_to_cell_text() {
        use pdf_oxide::structure::table_extractor::TableCell;

        let cell = TableCell {
            text: "  hello\nworld  ".to_string(),
            colspan: 1,
            rowspan: 1,
            mcids: vec![],
            spans: vec![],
            bbox: None,
            is_header: false,
        };

        let text = cell_text_in_reading_order(&cell);
        assert_eq!(
            text, "hello world",
            "fallback must trim and collapse newlines; got: {text:?}"
        );
    }

    /// Build a minimal single-page PDF with a 2-column stroke-bordered table.
    ///
    /// 5 rows × 2 columns. Every cell boundary is drawn with stroke_rect/stroke_line
    /// so pdf_oxide's detect_tables_with_lines can lock onto the paths.
    fn build_two_column_bordered_table_pdf() -> Vec<u8> {
        use pdf_oxide::geometry::Rect;
        use pdf_oxide::writer::{DocumentBuilder, LineStyle, TextAlign};

        let style = LineStyle::new(1.0, 0.0, 0.0, 0.0);

        let mut doc = DocumentBuilder::new();
        doc.a4_page()
            .stroke_rect(50.0, 550.0, 350.0, 200.0, style.clone())
            .stroke_line(200.0, 550.0, 200.0, 750.0, style.clone())
            .stroke_line(50.0, 710.0, 400.0, 710.0, style.clone())
            .stroke_line(50.0, 670.0, 400.0, 670.0, style.clone())
            .stroke_line(50.0, 630.0, 400.0, 630.0, style.clone())
            .stroke_line(50.0, 590.0, 400.0, 590.0, style.clone())
            .text_in_rect(Rect::new(50.0, 710.0, 150.0, 40.0), "Item", TextAlign::Left)
            .text_in_rect(Rect::new(200.0, 710.0, 200.0, 40.0), "Status", TextAlign::Left)
            .text_in_rect(Rect::new(50.0, 670.0, 150.0, 40.0), "8", TextAlign::Left)
            .text_in_rect(Rect::new(200.0, 670.0, 200.0, 40.0), "Not correct", TextAlign::Left)
            .text_in_rect(Rect::new(50.0, 630.0, 150.0, 40.0), "27", TextAlign::Left)
            .text_in_rect(Rect::new(200.0, 630.0, 200.0, 40.0), "Incomplete", TextAlign::Left)
            .text_in_rect(Rect::new(50.0, 590.0, 150.0, 40.0), "29,30", TextAlign::Left)
            .text_in_rect(Rect::new(200.0, 590.0, 200.0, 40.0), "Missing data", TextAlign::Left)
            .text_in_rect(Rect::new(50.0, 550.0, 150.0, 40.0), "45", TextAlign::Left)
            .text_in_rect(Rect::new(200.0, 550.0, 200.0, 40.0), "Fixed", TextAlign::Left)
            .done();
        doc.build().expect("DocumentBuilder must produce valid PDF bytes")
    }

    /// `extract_tables_native` (strict, min_table_columns=3) must NOT detect a
    /// 2-column bordered table. Regression baseline for issue #964.
    #[test]
    fn extract_tables_native_misses_two_column_bordered_table() {
        let bytes = build_two_column_bordered_table_pdf();
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open synthetic PDF");
        let tables = extract_tables_native(&mut doc).expect("extract_tables_native must not error");
        assert!(
            tables.is_empty(),
            "extract_tables_native (strict, min 3 cols) must not detect a 2-column table; got: {tables:?}"
        );
    }

    /// `extract_tables_bordered` (relaxed, min_table_columns=2) must detect the
    /// 2-column stroke-bordered table that `extract_tables_native` skips.
    #[test]
    fn extract_tables_bordered_detects_two_column_bordered_table() {
        let bytes = build_two_column_bordered_table_pdf();
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open synthetic PDF");
        let skip = HashSet::new();
        let tables = extract_tables_bordered(&mut doc, &skip).expect("extract_tables_bordered must not error");
        assert!(
            !tables.is_empty(),
            "extract_tables_bordered must detect the 2-column stroke-bordered table"
        );
        let table = &tables[0];
        assert_eq!(table.cells.len(), 5, "expected 5 rows, got {}", table.cells.len());
        assert!(
            table.cells.iter().all(|row| row.len() == 2),
            "all rows must have 2 columns; rows: {:?}",
            table.cells.iter().map(|r| r.len()).collect::<Vec<_>>()
        );
        assert_eq!(table.page_number, 1);
        assert!(!table.markdown.trim().is_empty(), "must produce non-empty markdown");
    }

    /// `extract_tables_bordered` must skip pages listed in `skip_pages`.
    #[test]
    fn extract_tables_bordered_skip_pages_honored() {
        let bytes = build_two_column_bordered_table_pdf();
        let mut doc = OxideDocument::open_bytes(&bytes).expect("open synthetic PDF");
        let mut skip = HashSet::new();
        skip.insert(1u32);
        let tables = extract_tables_bordered(&mut doc, &skip).expect("extract_tables_bordered must not error");
        assert!(
            tables.is_empty(),
            "skip_pages={{1}} must suppress the only page; got: {tables:?}"
        );
    }
}
