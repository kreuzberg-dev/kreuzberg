//! Table reconstruction from PDF segments (no OCR dependency).
//!
//! This module provides table reconstruction utilities that work with any
//! source of word-level text data (PDF native text, OCR output, etc.).
//! It re-exports core types from `table_core` and adds PDF-specific
//! conversion helpers.

pub(crate) use crate::table_core::{HocrWord, reconstruct_table, table_to_markdown};

const DENSE_NUMERIC_MIN_DATA_ROWS: usize = 6;
const DENSE_NUMERIC_MIN_COLUMNS: usize = 6;
const DENSE_NUMERIC_MIN_CELL_PERCENT: usize = 75;
/// Minimum non-empty data cells for the short-numeric-table exemption. Below
/// this there is too little evidence to call a grid a genuine table.
const SHORT_NUMERIC_MIN_DATA_CELLS: usize = 4;
/// Data-cell numeric fraction at or above which a short, wide single-word grid
/// is a genuine numeric table (invoice line items, small metric tables) rather
/// than shredded multi-column prose. Prose columns are alphabetic, so this bar
/// is unreachable for the misparses the ≥5-column guard targets — it recovers
/// the short borderless tables the corrected preprocessing loses (#1316)
/// without reopening the #36 fabrication hole.
const SHORT_NUMERIC_MIN_CELL_PERCENT: usize = 60;
const SHORT_NUMERIC_MIN_ROW_OCCUPANCY_PERCENT: usize = 85;
/// Short, wide grids have too few rows to establish stable column structure,
/// so require slightly denser evidence than the general table validator.
const SHORT_WIDE_MAX_DATA_ROWS: usize = 2;
const SHORT_WIDE_MIN_COLUMNS: usize = 6;
const SHORT_WIDE_MAX_EMPTY_CELL_PERCENT: usize = 35;
const LARGE_TABLE_MIN_COLUMNS: usize = 6;
const DEFAULT_MIN_DATA_ROW_DIGIT_CELLS: usize = 3;
const REPEATED_DATA_ROW_COUNT: usize = 3;
const ROW_SHAPE_MIN_OVERLAP_PERCENT: usize = 80;
const DENSE_SCALAR_MIN_DATA_ROWS: usize = 20;
const DENSE_SCALAR_MIN_COLUMNS: usize = 6;
const DENSE_SCALAR_MIN_FILLED_PERCENT: usize = 75;
const DENSE_SCALAR_MIN_COMPACT_PERCENT: usize = 90;
const DENSE_SCALAR_MIN_DIGIT_PERCENT: usize = 25;
const DENSE_SCALAR_MAX_CELL_CHARS: usize = 24;
const SPURIOUS_COLUMN_MIN_DATA_ROWS: usize = 20;
const SPURIOUS_COLUMN_MIN_COLUMNS: usize = 6;
const SPURIOUS_COLUMN_MIN_RETAINED_DENSITY_PERCENT: usize = 75;
const FOOTER_MIN_ALPHA_PERCENT: usize = 70;

#[cfg(feature = "pdf")]
use super::hierarchy::SegmentData;

/// Convert a PDF `SegmentData` to an `HocrWord` for table reconstruction.
///
/// `SegmentData` uses PDF coordinates (y=0 at bottom, increases upward).
/// `HocrWord` uses image coordinates (y=0 at top, increases downward).
///
/// For a segment drawn on a rotated text matrix (GH#1358: a sideways table),
/// `seg.x`/`seg.y` are the run's *page-space* origin, but the run's own row
/// axis and column axis are rotated relative to the page — using raw
/// page-space x/y here would group a rotated table's cells into rows and
/// columns along the wrong axes. [`SegmentData::upright_origin`] rotates the
/// origin back into the segment's own reading frame (identity for the
/// unrotated case, matching the plain x/y this replaced) so the row/column
/// clustering downstream in `oxide::table::cluster_words_into_vertical_regions`
/// operates on the table's actual advance/cross axes instead of the page's.
#[cfg(feature = "pdf")]
pub(crate) fn segment_to_hocr_word(seg: &SegmentData, page_height: f32) -> HocrWord {
    let (advance, cross) = seg.upright_origin();
    let top_image = (page_height - (cross + seg.height)).round().max(0.0) as u32;
    HocrWord {
        text: seg.text.clone(),
        left: advance.round().max(0.0) as u32,
        top: top_image,
        width: seg.width.round().max(0.0) as u32,
        height: seg.height.round().max(0.0) as u32,
        confidence: 95.0,
    }
}

/// Split a `SegmentData` into word-level `HocrWord`s for table reconstruction.
///
/// Pdfium segments can contain multiple whitespace-separated words (merged by
/// shared baseline + font). For table cell matching, each word needs its own
/// bounding box so it can be assigned to the correct column/cell.
///
/// Single-word segments use `segment_to_hocr_word` directly (fast path).
/// Multi-word segments get proportional bbox estimation per word based on
/// byte offset within the segment text.
#[cfg(feature = "pdf")]
pub(crate) fn split_segment_to_words(seg: &SegmentData, page_height: f32) -> Vec<HocrWord> {
    let trimmed = seg.text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if !trimmed.contains(char::is_whitespace) {
        return vec![segment_to_hocr_word(seg, page_height)];
    }

    let text = &seg.text;
    let total_bytes = text.len() as f32;
    if total_bytes <= 0.0 {
        return Vec::new();
    }

    // See `segment_to_hocr_word` for why the segment's own upright frame
    // (rather than raw page-space x/y) is used here: `advance` is the
    // position along the run's reading axis, which per-word interpolation
    // below advances along via `frac_start * seg.width` — consistent
    // whether or not the run is rotated.
    let (advance, cross) = seg.upright_origin();
    let top_image = (page_height - (cross + seg.height)).round().max(0.0) as u32;
    let seg_height = seg.height.round().max(0.0) as u32;

    let mut words = Vec::new();
    let mut search_start = 0;
    for word in text.split_whitespace() {
        let byte_offset = text[search_start..].find(word).map(|pos| search_start + pos);
        let Some(offset) = byte_offset else {
            continue;
        };
        search_start = offset + word.len();

        let frac_start = offset as f32 / total_bytes;
        let frac_width = word.len() as f32 / total_bytes;

        words.push(HocrWord {
            text: word.to_string(),
            left: (advance + frac_start * seg.width).round().max(0.0) as u32,
            top: top_image,
            width: (frac_width * seg.width).round().max(1.0) as u32,
            height: seg_height,
            confidence: 95.0,
        });
    }

    words
}

/// Convert a page's segments to word-level `HocrWord`s for table extraction.
///
/// Splits multi-word segments into individual words with proportional bounding
/// boxes, ensuring each word can be independently matched to table cells.
#[cfg(feature = "pdf")]
pub(crate) fn segments_to_words(segments: &[SegmentData], page_height: f32) -> Vec<HocrWord> {
    segments
        .iter()
        .flat_map(|seg| split_segment_to_words(seg, page_height))
        .collect()
}

/// Column-wise merge of several table rows into a single logical row.
///
/// Each output column's text is the space-joined concatenation of that
/// column's non-empty cells across `rows`, in row order, truncated to
/// `column_count` columns. Used to collapse a fragment's word-wrapped header
/// sub-lines into one header row here, and reused by
/// [`super::structure::pipeline`]'s table-continuation stitching to collapse
/// a whole table fragment (whose rows are word-wrapped sub-lines of a single
/// logical row, once `oxide::table`'s row-gap clustering has split one
/// physical table into several fragments) into one row when the fragments are
/// stitched back together.
pub(crate) fn merge_rows_columnwise(rows: &[Vec<String>], column_count: usize) -> Vec<String> {
    let mut merged = vec![String::new(); column_count];
    for row in rows {
        for (idx, cell) in row.iter().enumerate().take(column_count) {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !merged[idx].is_empty() {
                merged[idx].push(' ');
            }
            merged[idx].push_str(trimmed);
        }
    }
    merged
}

/// Post-process a raw table grid to validate structure and clean up.
///
/// Returns `None` if the table fails structural validation.
///
/// When `layout_guided` is true, the layout model already confirmed this is
/// a table, so validation thresholds are relaxed:
/// - Minimum columns: 3 → 2
/// - Column sparsity: 75% → 95%
/// - Overall density: 40% → 15%
/// - Prose detection: reject if >70% cells >100 chars (vs >50% >60 chars)
/// - Prose detection: reject if avg cell >80 chars (vs >50 chars)
/// - Single-word cell: reject if >85% single-word (vs >70%)
/// - Content asymmetry: reject if one col >92% of text (vs >85%)
/// - Column-text-flow: applied equally (reject if >60% rows flow through)
pub(crate) fn post_process_table(
    table: Vec<Vec<String>>,
    layout_guided: bool,
    allow_single_column: bool,
) -> Option<Vec<Vec<String>>> {
    let min_columns = if allow_single_column {
        1
    } else if layout_guided {
        2
    } else {
        3
    };
    post_process_table_inner(table, min_columns, layout_guided)
}

fn post_process_table_inner(
    mut table: Vec<Vec<String>>,
    min_columns: usize,
    layout_guided: bool,
) -> Option<Vec<Vec<String>>> {
    table.retain(|row| row.iter().any(|cell| !cell.trim().is_empty()));
    if table.is_empty() {
        return None;
    }

    let mut non_empty = 0usize;
    let mut long_cells = 0usize;
    let mut total_chars = 0usize;
    for row in &table {
        for cell in row {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            let char_count = trimmed.chars().count();
            non_empty += 1;
            total_chars += char_count;
            if char_count > 60 {
                long_cells += 1;
            }
        }
    }

    if non_empty > 0 {
        if layout_guided {
            if long_cells > 0 {
                let long_cells_100 = table
                    .iter()
                    .flat_map(|row| row.iter())
                    .filter(|cell| {
                        let trimmed = cell.trim();
                        !trimmed.is_empty() && trimmed.chars().count() > 100
                    })
                    .count();
                if long_cells_100 * 10 > non_empty * 7 {
                    return None;
                }
            }
            if total_chars / non_empty > 80 {
                return None;
            }
        } else {
            if long_cells * 2 > non_empty {
                return None;
            }
            if total_chars / non_empty > 50 {
                return None;
            }
        }
    }

    let col_count = table.first().map_or(0, Vec::len);
    if col_count < min_columns {
        return None;
    }

    let data_start = find_data_start(&table, layout_guided);

    let mut header_rows = if data_start > 0 {
        table[..data_start].to_vec()
    } else {
        Vec::new()
    };
    let mut data_rows = table[data_start..].to_vec();

    if header_rows.len() > 2 {
        header_rows = header_rows[header_rows.len() - 2..].to_vec();
    }

    if header_rows.is_empty() {
        if data_rows.len() < 2 {
            return None;
        }
        header_rows.push(data_rows[0].clone());
        data_rows = data_rows[1..].to_vec();
    }

    let column_count = header_rows.first().or_else(|| data_rows.first()).map_or(0, Vec::len);

    if column_count == 0 {
        return None;
    }

    let header = merge_rows_columnwise(&header_rows, column_count);

    let mut processed = Vec::new();
    processed.push(header);
    processed.extend(data_rows);

    if processed.len() <= 1 {
        return None;
    }

    let mut col = 0;
    while col < processed[0].len() {
        let header_text = processed[0][col].trim().to_string();
        let data_empty = processed[1..]
            .iter()
            .all(|row| row.get(col).is_none_or(|cell| cell.trim().is_empty()));

        if data_empty {
            merge_header_only_column(&mut processed, col, header_text);
        } else {
            col += 1;
        }

        if processed.is_empty() || processed[0].is_empty() {
            return None;
        }
    }

    if processed[0].len() < 2 || processed.len() <= 1 {
        return None;
    }

    prune_spurious_interior_column(&mut processed, layout_guided);

    let data_row_count = processed.len() - 1;
    if data_row_count > 0 {
        for c in 0..processed[0].len() {
            let empty_count = processed[1..]
                .iter()
                .filter(|row| row.get(c).is_none_or(|cell| cell.trim().is_empty()))
                .count();
            let too_sparse = if layout_guided {
                empty_count * 20 > data_row_count * 19
            } else {
                empty_count * 4 > data_row_count * 3
            };
            if too_sparse {
                return None;
            }
        }
    }

    {
        let total_data_cells = data_row_count * processed[0].len();
        if total_data_cells > 0 {
            let filled = processed[1..]
                .iter()
                .flat_map(|row| row.iter())
                .filter(|cell| !cell.trim().is_empty())
                .count();
            let too_sparse = if layout_guided {
                filled * 20 < total_data_cells * 3
            } else {
                filled * 5 < total_data_cells * 2
            };
            if too_sparse {
                return None;
            }
        }
    }

    let dense_numeric_grid = is_dense_numeric_grid(&processed);

    if processed[0].len() >= 5 {
        let mut single_word_cells = 0usize;
        let mut non_empty_cells = 0usize;
        for row in processed.iter().skip(1) {
            for cell in row {
                let trimmed = cell.trim();
                if trimmed.is_empty() {
                    continue;
                }
                non_empty_cells += 1;
                let word_count = trimmed.split_whitespace().count();
                if word_count <= 2 {
                    single_word_cells += 1;
                }
            }
        }
        let threshold = if layout_guided { 85 } else { 70 };
        let dense_scalar_grid = layout_guided && is_dense_scalar_grid(&processed);
        if !dense_numeric_grid
            && !dense_scalar_grid
            && !is_predominantly_numeric_short_grid(&processed)
            && non_empty_cells >= 6
            && single_word_cells * 100 > non_empty_cells * threshold
        {
            return None;
        }
    }

    if processed[0].len() >= 2 {
        let mut flow_rows = 0usize;
        let mut eligible_rows = 0usize;
        for row in processed.iter().skip(1) {
            let col0 = row.first().map(|s| s.trim()).unwrap_or("");
            let col1 = row.get(1).map(|s| s.trim()).unwrap_or("");
            if col0.is_empty() || col1.is_empty() {
                continue;
            }
            eligible_rows += 1;
            let ends_without_punct =
                !col0.ends_with('.') && !col0.ends_with('?') && !col0.ends_with('!') && !col0.ends_with(':');
            let starts_lowercase = col1.chars().next().is_some_and(|c| c.is_lowercase());
            if ends_without_punct && starts_lowercase {
                flow_rows += 1;
            }
        }
        if eligible_rows >= 3 && flow_rows * 10 > eligible_rows * 6 {
            return None;
        }
    }

    {
        let num_cols = processed[0].len();
        let col_char_counts: Vec<usize> = (0..num_cols)
            .map(|c| {
                processed[1..]
                    .iter()
                    .map(|row| row.get(c).map_or(0, |cell| cell.trim().len()))
                    .sum()
            })
            .collect();
        let total_chars_asym: usize = col_char_counts.iter().sum();

        if total_chars_asym > 0 {
            let max_col_share = col_char_counts
                .iter()
                .map(|&cc| cc as f64 / total_chars_asym as f64)
                .fold(0.0_f64, f64::max);
            let dominant_threshold = if layout_guided { 0.92 } else { 0.85 };
            if max_col_share > dominant_threshold {
                return None;
            }

            if !layout_guided {
                for (c, &col_chars) in col_char_counts.iter().enumerate() {
                    let char_share = col_chars as f64 / total_chars_asym as f64;
                    let empty_in_col = processed[1..]
                        .iter()
                        .filter(|row| row.get(c).is_none_or(|cell| cell.trim().is_empty()))
                        .count();
                    let empty_ratio = empty_in_col as f64 / data_row_count as f64;

                    if char_share < 0.15 && empty_ratio > 0.5 {
                        return None;
                    }
                }
            }
        }
    }

    if processed.len() > 3 && processed[0].len() >= 2 {
        let last_col = processed[0].len() - 1;
        let mut continuation_count = 0usize;
        let mut eligible_transitions = 0usize;
        for pair in processed[1..].windows(2) {
            let prev_last = pair[0].get(last_col).map(|s| s.trim()).unwrap_or("");
            let next_first = pair[1].first().map(|s| s.trim()).unwrap_or("");
            if prev_last.is_empty() || next_first.is_empty() {
                continue;
            }
            eligible_transitions += 1;
            let ends_without_punct = !prev_last.ends_with('.')
                && !prev_last.ends_with('?')
                && !prev_last.ends_with('!')
                && !prev_last.ends_with(':')
                && !prev_last.ends_with(';');
            let starts_lowercase = next_first.chars().next().is_some_and(|c| c.is_lowercase());
            if ends_without_punct && starts_lowercase {
                continuation_count += 1;
            }
        }
        if eligible_transitions >= 3 && continuation_count * 10 > eligible_transitions * 4 {
            return None;
        }
    }

    {
        let num_cols = processed[0].len();
        let num_data_rows = processed.len() - 1;
        if num_data_rows > 20 && num_cols <= 3 {
            let total_data_cells = num_data_rows * num_cols;
            let filled_cells = processed[1..]
                .iter()
                .flat_map(|row| row.iter())
                .filter(|cell| !cell.trim().is_empty())
                .count();
            if total_data_cells > 0
                && filled_cells * 100 > total_data_cells * 80
                && looks_like_prose_in_columns(&processed[1..], num_cols)
            {
                return None;
            }
        }
    }

    {
        let num_cols = processed[0].len();
        let num_data_rows = processed.len() - 1;
        if (3..=5).contains(&num_cols) && num_data_rows >= 5 {
            let col_avg_lengths: Vec<f64> = (0..num_cols)
                .map(|c| {
                    let mut total_len = 0usize;
                    let mut count = 0usize;
                    for row in processed.iter().skip(1) {
                        let cell = row.get(c).map(|s| s.trim()).unwrap_or("");
                        if !cell.is_empty() {
                            total_len += cell.len();
                            count += 1;
                        }
                    }
                    if count > 0 {
                        total_len as f64 / count as f64
                    } else {
                        0.0
                    }
                })
                .collect();

            let text_col_avgs: Vec<f64> = col_avg_lengths.iter().copied().filter(|&avg| avg > 15.0).collect();

            if text_col_avgs.len() >= 3 {
                let min_avg = text_col_avgs.iter().copied().fold(f64::INFINITY, f64::min);
                let max_avg = text_col_avgs.iter().copied().fold(0.0_f64, f64::max);

                if min_avg > 0.0 && max_avg <= min_avg * 2.0 {
                    let total_data_cells = num_data_rows * num_cols;
                    let filled_cells = processed[1..]
                        .iter()
                        .flat_map(|row| row.iter())
                        .filter(|cell| !cell.trim().is_empty())
                        .count();
                    let fill_rate = filled_cells as f64 / total_data_cells as f64;
                    if fill_rate > 0.75 {
                        return None;
                    }
                }
            }
        }
    }

    for cell in &mut processed[0] {
        let text = cell.trim().replace("  ", " ");
        *cell = text;
    }

    for row in processed.iter_mut().skip(1) {
        for cell in row.iter_mut() {
            normalize_data_cell(cell);
        }
    }

    Some(processed)
}

fn find_data_start(table: &[Vec<String>], layout_guided: bool) -> usize {
    let first_numeric_row = table
        .iter()
        .position(|row| digit_cell_count(row) >= DEFAULT_MIN_DATA_ROW_DIGIT_CELLS)
        .unwrap_or(0);
    let column_count = table.first().map_or(0, Vec::len);
    if !layout_guided || column_count < LARGE_TABLE_MIN_COLUMNS || table.len() < REPEATED_DATA_ROW_COUNT {
        return first_numeric_row;
    }

    let repeated_start = table.windows(REPEATED_DATA_ROW_COUNT).position(|rows| {
        rows.iter()
            .all(|row| digit_cell_count(row) >= DEFAULT_MIN_DATA_ROW_DIGIT_CELLS)
            && rows.windows(2).all(|pair| row_shapes_match(&pair[0], &pair[1]))
    });
    repeated_start
        .filter(|&start| {
            start == first_numeric_row
                || looks_like_multiline_numeric_header(&table[first_numeric_row], &table[first_numeric_row + 1..start])
        })
        .unwrap_or(first_numeric_row)
}

fn looks_like_multiline_numeric_header(header: &[String], continuation_rows: &[Vec<String>]) -> bool {
    let filled_header_cells = header.iter().filter(|cell| !cell.trim().is_empty()).count();
    let multiword_labels = header
        .iter()
        .filter(|cell| {
            let text = cell.trim();
            text.split_whitespace().count() >= 2 && text.chars().any(char::is_alphabetic)
        })
        .count();
    let continuation_cells: Vec<&str> = continuation_rows
        .iter()
        .flat_map(|row| row.iter())
        .map(|cell| cell.trim())
        .filter(|cell| !cell.is_empty())
        .collect();
    let has_parenthesized_unit = continuation_cells
        .iter()
        .any(|cell| cell.starts_with('(') && cell.contains(')'));

    !continuation_rows.is_empty()
        && multiword_labels >= 2
        && continuation_cells.len() < filled_header_cells
        && has_parenthesized_unit
}

fn digit_cell_count(row: &[String]) -> usize {
    row.iter()
        .filter(|cell| cell.chars().any(|character| character.is_ascii_digit()))
        .count()
}

fn row_shapes_match(left: &[String], right: &[String]) -> bool {
    let column_count = left.len().max(right.len());
    let mut occupied_union = 0usize;
    let mut occupied_intersection = 0usize;
    for column in 0..column_count {
        let left_filled = left.get(column).is_some_and(|cell| !cell.trim().is_empty());
        let right_filled = right.get(column).is_some_and(|cell| !cell.trim().is_empty());
        occupied_union += usize::from(left_filled || right_filled);
        occupied_intersection += usize::from(left_filled && right_filled);
    }
    occupied_union > 0
        && occupied_intersection.saturating_mul(100) >= occupied_union.saturating_mul(ROW_SHAPE_MIN_OVERLAP_PERCENT)
}

/// Remove one empty-header interior track that only catches a stray word in a
/// large, otherwise dense layout-guided table. Such tracks arise when a footer
/// word has an x-position that does not occur in the table body.
fn prune_spurious_interior_column(table: &mut [Vec<String>], layout_guided: bool) -> bool {
    let Some(header) = table.first() else {
        return false;
    };
    let column_count = header.len();
    let data_row_count = table.len().saturating_sub(1);
    if !layout_guided || column_count < SPURIOUS_COLUMN_MIN_COLUMNS || data_row_count < SPURIOUS_COLUMN_MIN_DATA_ROWS {
        return false;
    }

    let candidates: Vec<usize> = (1..column_count - 1)
        .filter(|&column| header[column].trim().is_empty())
        .filter(|&column| {
            let populated_rows: Vec<usize> = table[1..]
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    row.get(column)
                        .is_some_and(|cell| !cell.trim().is_empty())
                        .then_some(index)
                })
                .collect();
            populated_rows.as_slice() == [data_row_count - 1]
                && table.last().is_some_and(|row| looks_like_footer_row(row))
        })
        .collect();
    let [column] = candidates.as_slice() else {
        return false;
    };

    let retained_cells = data_row_count.saturating_mul(column_count - 1);
    let retained_filled = table[1..]
        .iter()
        .flat_map(|row| row.iter().enumerate())
        .filter(|(index, cell)| *index != *column && !cell.trim().is_empty())
        .count();
    if retained_cells == 0
        || retained_filled.saturating_mul(100)
            < retained_cells.saturating_mul(SPURIOUS_COLUMN_MIN_RETAINED_DENSITY_PERCENT)
    {
        return false;
    }

    merge_interior_column(table, *column);
    true
}

fn looks_like_footer_row(row: &[String]) -> bool {
    let non_empty: Vec<&str> = row
        .iter()
        .map(|cell| cell.trim())
        .filter(|cell| !cell.is_empty())
        .collect();
    if non_empty.len() < 2 || !non_empty.iter().any(|cell| cell.split_whitespace().count() >= 2) {
        return false;
    }
    let text = non_empty.join(" ");
    let alphanumeric = text.chars().filter(|character| character.is_alphanumeric()).count();
    let alphabetic = text.chars().filter(|character| character.is_alphabetic()).count();
    alphanumeric > 0 && alphabetic.saturating_mul(100) >= alphanumeric.saturating_mul(FOOTER_MIN_ALPHA_PERCENT)
}

fn merge_interior_column(table: &mut [Vec<String>], column: usize) {
    let left_occupancy = table[1..]
        .iter()
        .filter(|row| row.get(column - 1).is_some_and(|cell| !cell.trim().is_empty()))
        .count();
    let right_occupancy = table[1..]
        .iter()
        .filter(|row| row.get(column + 1).is_some_and(|cell| !cell.trim().is_empty()))
        .count();
    let merge_right = right_occupancy >= left_occupancy;

    for row in table {
        let text = row.remove(column).trim().to_string();
        if text.is_empty() {
            continue;
        }
        let target = if merge_right { column } else { column - 1 };
        let existing = row[target].trim();
        row[target] = if existing.is_empty() {
            text
        } else if merge_right {
            format!("{text} {existing}")
        } else {
            format!("{existing} {text}")
        };
    }
}

/// Minimum non-empty cells for [`looks_like_shredded_prose_row`] to consider a
/// row "densely filled" rather than a sparse real table row.
const SHREDDED_PROSE_MIN_FILLED_CELLS: usize = 4;
/// A shredded-prose cell averages this many words or fewer (unlike
/// `PROSE_WORDS_PER_CELL`'s phrase-per-cell prose, single-word cells here are
/// the row-shredding signal).
const SHREDDED_PROSE_MAX_AVG_WORDS_PER_CELL: f64 = 2.5;
/// Minimum concatenated row text length for [`looks_like_shredded_prose_row`]
/// to consider a row substantial enough to be a real clause rather than a
/// handful of short table values.
const SHREDDED_PROSE_MIN_ROW_TEXT_LEN: usize = 30;

/// Decide whether a single data row reads as one clause of a word-shredded,
/// semicolon-delimited prose list rather than genuine table data: most of the
/// row's columns are filled (a real table row from a word-wrapped table
/// fragment leaves many columns empty; a shredded sentence naturally
/// populates almost every column), the cells average few words each (mirrors
/// the one-word-per-cell splitting), the row reads as a substantial run of
/// text, and it ends on clause-terminal punctuation.
fn looks_like_shredded_prose_row(row: &[String], num_cols: usize) -> bool {
    let cells: Vec<&str> = row.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    if cells.len() < SHREDDED_PROSE_MIN_FILLED_CELLS {
        return false;
    }
    if num_cols == 0 || (cells.len() as f64) <= num_cols as f64 * 0.5 {
        return false;
    }

    let concatenated_len: usize = cells.iter().map(|c| c.len()).sum();
    if concatenated_len < SHREDDED_PROSE_MIN_ROW_TEXT_LEN {
        return false;
    }

    let total_words: usize = cells.iter().map(|c| c.split_whitespace().count()).sum();
    let avg_words = total_words as f64 / cells.len() as f64;
    if avg_words > SHREDDED_PROSE_MAX_AVG_WORDS_PER_CELL {
        return false;
    }

    cells
        .last()
        .is_some_and(|last| matches!(last.chars().last(), Some(';' | ':' | '.' | ',')))
}

/// Decide whether a dense grid of data rows is prose laid out in columns rather
/// than a real table. The signal is words-per-cell: a table cell holds a value (a
/// number, a code, a short label), while columned prose (a two-column article, a
/// wrapped paragraph) fills each cell with a phrase. This gates the density guard
/// so that a dense numeric ledger (Account | Amount | Note, 30+ rows) is not cut by
/// row-count alone; genuinely alphabetic prose is still caught downstream by the
/// alpha-ratio row-coherence check in `is_well_formed_table` (xberg-io/xberg#1223).
fn looks_like_prose_in_columns(data_rows: &[Vec<String>], num_cols: usize) -> bool {
    /// A cell averaging this many words or more reads as a phrase, not a value.
    const PROSE_WORDS_PER_CELL: f64 = 4.0;

    if num_cols < 2 {
        return false;
    }
    let mut prose_rows = 0usize;
    let mut eligible_rows = 0usize;
    for row in data_rows {
        let cells: Vec<&str> = row.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
        if cells.len() < 2 {
            continue;
        }
        let total_len: usize = cells.iter().map(|c| c.len()).sum();
        if total_len < 15 {
            continue;
        }
        eligible_rows += 1;
        let total_words: usize = cells.iter().map(|c| c.split_whitespace().count()).sum();
        let avg_words = total_words as f64 / cells.len() as f64;
        if avg_words >= PROSE_WORDS_PER_CELL {
            prose_rows += 1;
        }
    }
    eligible_rows >= 3 && prose_rows * 2 > eligible_rows
}

/// A cell containing at least one alphabetic run but not itself a numeric value.
/// Word cells are the signal of wrapped prose (as opposed to numeric table data)
/// when the grid's cells are too thin to average four words.
fn is_word_cell(cell: &str) -> bool {
    !is_numeric_value_cell(cell) && cell.chars().any(|c| c.is_alphabetic())
}

/// Decide whether a 1–2 data-row grid is really a wrapped-prose passage split
/// across columns rather than a genuine short table. This closes the short-grid
/// hole where the ≥3-row alpha guard, the ≥4-row uniformity/vocabulary guards,
/// and the shredded-prose branch (which demands *every* row end on clause-
/// terminal punctuation) all miss it, so it reaches `return true` and is
/// fabricated as a table (xberg-io/xberg#36).
///
/// Two prose shapes are detected, both applied per row:
/// - **phrase-per-cell** — cells average ≥ `PROSE_WORDS_PER_CELL` words and the
///   row is alphabetic (`alpha_ratio > 0.8`): columns of full phrases (a 2–5
///   column reflow of body text).
/// - **wide-shredded** — a wide row (≥ `MIN_SHREDDED_WORD_CELLS` filled cells)
///   of thin cells (≤ 2.5 words each) that are mostly word cells: a single
///   prose line chopped into one-or-two-word columns (the multi-column academic
///   misparse, e.g. arxiv 0903.1810).
///
/// Genuine short tables survive via a numeric-**fraction** exemption: a real
/// numeric table is mostly value cells, whereas prose that merely contains an
/// equation or a stray number is not. Requiring *every* eligible row to read as
/// prose is deliberately conservative — at 1–2 rows there is no cross-row
/// evidence to average over.
fn looks_like_short_columned_prose(data_rows: &[Vec<String>], num_cols: usize) -> bool {
    /// A cell averaging this many words or more reads as a phrase, not a value.
    /// Mirrors `PROSE_WORDS_PER_CELL` in [`looks_like_prose_in_columns`].
    const SHORT_PROSE_WORDS_PER_CELL: f64 = 4.0;
    /// Above this alphabetic+whitespace fraction a phrase row reads as prose.
    /// Mirrors the alpha-ratio cutoff in [`is_well_formed_table`].
    const SHORT_PROSE_ALPHA_RATIO: f64 = 0.8;
    /// Minimum concatenated row text length to be eligible. Mirrors the 15-char
    /// floor in [`looks_like_prose_in_columns`].
    const SHORT_PROSE_MIN_CONCAT_LEN: usize = 15;
    /// A numeric-value cell fraction at or above this keeps the grid: a genuine
    /// short table is mostly values; prose with an incidental number is not.
    const SHORT_PROSE_NUMERIC_EXEMPT_PERCENT: usize = 30;
    /// A shredded row needs at least this many filled cells — narrow grids are
    /// left to the phrase-per-cell shape so 2-column key/value stays a table.
    const MIN_SHREDDED_WORD_CELLS: usize = 4;
    /// A shredded row's cells average at most this many words (one-or-two-word
    /// fragments). Mirrors `SHREDDED_PROSE_MAX_AVG_WORDS_PER_CELL`.
    const SHREDDED_MAX_AVG_WORDS: f64 = 2.5;
    /// At least this fraction of a shredded row's filled cells must be word
    /// cells (not numbers) for it to read as prose rather than a numeric row.
    const SHREDDED_MIN_WORD_CELL_FRACTION: f64 = 0.6;

    if num_cols < 2 {
        return false;
    }

    let mut filled_cells = 0usize;
    let mut numeric_value_cells = 0usize;
    for row in data_rows {
        for cell in row {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            filled_cells += 1;
            if is_numeric_value_cell(trimmed) {
                numeric_value_cells += 1;
            }
        }
    }
    if filled_cells == 0 {
        return false;
    }
    if numeric_value_cells * 100 >= filled_cells * SHORT_PROSE_NUMERIC_EXEMPT_PERCENT {
        return false;
    }

    let mut eligible_rows = 0usize;
    let mut prose_rows = 0usize;
    for row in data_rows {
        let cells: Vec<&str> = row.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
        if cells.len() < 2 {
            continue;
        }
        let concatenated = cells.join(" ");
        if concatenated.len() < SHORT_PROSE_MIN_CONCAT_LEN {
            continue;
        }
        eligible_rows += 1;

        let total_words: usize = cells.iter().map(|c| c.split_whitespace().count()).sum();
        let avg_words = total_words as f64 / cells.len() as f64;
        let alpha_ratio = {
            let alpha = concatenated
                .chars()
                .filter(|c| c.is_alphabetic() || c.is_whitespace())
                .count();
            alpha as f64 / concatenated.len() as f64
        };
        let is_phrase_prose = avg_words >= SHORT_PROSE_WORDS_PER_CELL && alpha_ratio > SHORT_PROSE_ALPHA_RATIO;

        let word_cells = cells.iter().filter(|c| is_word_cell(c)).count();
        let is_shredded_prose = cells.len() >= MIN_SHREDDED_WORD_CELLS
            && avg_words <= SHREDDED_MAX_AVG_WORDS
            && word_cells as f64 >= cells.len() as f64 * SHREDDED_MIN_WORD_CELL_FRACTION;

        if is_phrase_prose || is_shredded_prose {
            prose_rows += 1;
        }
    }

    eligible_rows >= 1 && prose_rows * 2 > eligible_rows
}

/// Validate whether a reconstructed table grid represents a well-formed table
/// rather than multi-column prose or a repeated page element.
///
/// Returns `true` if the grid looks like a real table, `false` if it should be
/// rejected and its content emitted as paragraph text instead.
///
/// The checks catch cases the layout model misidentifies as tables:
/// - Multi-column prose split into a grid (detected via row coherence and column uniformity)
/// - Repeated page elements (headers/footers detected as tables on every page)
/// - Low-vocabulary repetitive content (same few words in every row)
pub(crate) fn is_well_formed_table(grid: &[Vec<String>]) -> bool {
    is_well_formed_table_core(grid, false)
}

/// Share of (column boundary, row) pairs whose boundary has a word running
/// across it, above which a rule-less candidate is treated as prose (#1399).
///
/// Deliberately NOT the ~30% the issue proposed. Measured over 147 genuine
/// ruled-table regions from `test_documents/pdf/`, each scored on its own
/// bounding box (the reconstructor only ever sees a pre-segmented region, so
/// scoring whole pages mixes in surrounding prose and inflates the ratio):
/// min 11.9%, median 33.0%, p90 45.6%, max 74.7%. A 30% cut rejects 94 of
/// those 147 real tables; 50% rejects 8; 60% rejects 4. The #1399 prose region
/// scores 65.0%, and every prose page in that document scores 47.8-75.5%, so
/// 60% separates the reported defect from real tables with the least collateral
/// damage this signal can achieve on its own. It is deliberately a weak gate:
/// the ruling-line check below is the load-bearing one. ~keep
const MAX_STRADDLED_BOUNDARY_RATIO: f64 = 0.60;

/// Row-grouping tolerance as a multiple of median word height, matching the
/// value `reconstruct_table` itself uses so both see the same rows.
const STRADDLE_ROW_THRESHOLD_RATIO: f64 = 0.5;

/// Fraction of (column boundary, row) pairs crossed by a word's bounding box.
///
/// A column boundary is the *start* of the next column, not the midpoint
/// between two column positions. [`crate::table_core::detect_columns`] returns
/// each column's **median left edge**, so a midpoint between two such medians
/// falls inside the left column's own text rather than in the gutter, and any
/// word wider than half the column pitch straddles it — flagging legitimate
/// tables that merely contain one long word.
///
/// Measured per row rather than over the whole region: the issue's definition
/// is that a column boundary is a vertical band of whitespace which holds on
/// every row, so text running across it *on most rows* is what disqualifies it.
/// A single long word on one row is not evidence of prose.
pub(crate) fn straddled_boundary_ratio(region: &[HocrWord], column_positions: &[u32]) -> f64 {
    if column_positions.len() < 2 || region.is_empty() {
        return 0.0;
    }

    let row_positions = crate::table_core::detect_rows(region, STRADDLE_ROW_THRESHOLD_RATIO);
    if row_positions.is_empty() {
        return 0.0;
    }

    let mut rows: Vec<Vec<&HocrWord>> = vec![Vec::new(); row_positions.len()];
    for word in region {
        let y_center = word.y_center() as u32;
        let Some((index, _)) = row_positions
            .iter()
            .enumerate()
            .min_by_key(|&(_, row_y)| row_y.abs_diff(y_center))
        else {
            continue;
        };
        rows[index].push(word);
    }

    let mut total = 0usize;
    let mut straddled = 0usize;
    for &boundary in &column_positions[1..] {
        for row in &rows {
            total += 1;
            if row
                .iter()
                .any(|word| word.left < boundary && word.left.saturating_add(word.width) > boundary)
            {
                straddled += 1;
            }
        }
    }

    if total == 0 {
        return 0.0;
    }
    straddled as f64 / total as f64
}

/// Well-formedness gate for the borderless heuristic path, which is the only
/// caller holding raw word geometry and the page's drawn-rule count.
///
/// Implements the two-signal admission test from xberg-io/xberg#1399:
///
/// 1. **Drawn ruling lines (positive).** A page whose region carries horizontal
///    rules had a producer that drew a table, so the candidate is admitted. The
///    reporter's survey found no real table lacking rules, and this is the
///    strong signal — see [`MAX_STRADDLED_BOUNDARY_RATIO`] for why the
///    geometric signal alone cannot carry the decision.
/// 2. **Are the column boundaries actually whitespace (fallback)?** With no
///    rules to go on, test the definition of a column. Continuous prose that
///    merely aligns into column-like x-buckets has words running across those
///    boundaries on most rows; a real borderless table does not, because its
///    cells do not overlap.
///
/// This only ever narrows acceptance relative to [`is_well_formed_table`].
pub(crate) fn is_well_formed_borderless_table(
    grid: &[Vec<String>],
    region: &[HocrWord],
    column_positions: &[u32],
    horizontal_rules: usize,
) -> bool {
    if !is_well_formed_table(grid) {
        return false;
    }
    if horizontal_rules > 0 {
        return true;
    }
    straddled_boundary_ratio(region, column_positions) < MAX_STRADDLED_BOUNDARY_RATIO
}

/// Core well-formedness check. `skip_columnar_prose_guard` drops only the
/// uniform-column-length prose heuristic, for callers that have already vetted
/// the region's columnar structure geometrically (the #1319 text-heavy geometric
/// fallback). A genuine key-value grid has regular, short column lengths that
/// this heuristic mistakes for wrapped columnar prose; every other structural
/// guard (empty-cell fraction, shredded-row, alpha-ratio, unique-word, and
/// header-duplication checks) still applies.
pub(crate) fn is_well_formed_table_core(grid: &[Vec<String>], skip_columnar_prose_guard: bool) -> bool {
    if grid.len() < 2 {
        return false;
    }
    let num_cols = grid[0].len();
    if num_cols < 2 {
        return false;
    }
    let dense_numeric_grid = is_dense_numeric_grid(grid);

    const DEFAULT_MAX_EMPTY_CELL_PERCENT: usize = 40;
    let data_row_count = grid.len().saturating_sub(1);
    let max_empty_cell_percent =
        if data_row_count <= SHORT_WIDE_MAX_DATA_ROWS && num_cols >= SHORT_WIDE_MIN_COLUMNS && !dense_numeric_grid {
            SHORT_WIDE_MAX_EMPTY_CELL_PERCENT
        } else {
            DEFAULT_MAX_EMPTY_CELL_PERCENT
        };
    let max_cols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    let total_cells = grid.len() * max_cols;
    if total_cells > 0 {
        let empty_cells = grid.len() * max_cols
            - grid
                .iter()
                .flat_map(|row| row.iter())
                .filter(|cell| !cell.trim().is_empty())
                .count();
        if empty_cells * 100 > total_cells * max_empty_cell_percent {
            return false;
        }
    }

    let data_rows = &grid[1..];

    if (1..3).contains(&data_rows.len()) && num_cols >= LARGE_TABLE_MIN_COLUMNS && !dense_numeric_grid {
        let shredded_rows = data_rows
            .iter()
            .filter(|row| looks_like_shredded_prose_row(row, num_cols))
            .count();
        if shredded_rows == data_rows.len() {
            return false;
        }
    }

    if !data_rows.is_empty()
        && num_cols >= 2
        && !dense_numeric_grid
        && looks_like_short_columned_prose(data_rows, num_cols)
    {
        return false;
    }

    if data_rows.len() >= 3 && num_cols >= 2 {
        let mut prose_like_rows = 0usize;
        let mut eligible_rows = 0usize;

        for row in data_rows {
            let concatenated: String = row
                .iter()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if concatenated.len() < 15 {
                continue;
            }
            eligible_rows += 1;

            let alpha_ratio = {
                let alpha = concatenated
                    .chars()
                    .filter(|c| c.is_alphabetic() || c.is_whitespace())
                    .count();
                alpha as f64 / concatenated.len() as f64
            };
            if alpha_ratio > 0.8 {
                prose_like_rows += 1;
            }
        }

        if eligible_rows >= 3 && prose_like_rows * 2 > eligible_rows {
            return false;
        }
    }

    if num_cols >= 3 && data_rows.len() >= 4 {
        let col_stats: Vec<(f64, f64)> = (0..num_cols)
            .map(|c| {
                let lengths: Vec<f64> = data_rows
                    .iter()
                    .filter_map(|row| {
                        let cell = row.get(c).map(|s| s.trim()).unwrap_or("");
                        if cell.is_empty() { None } else { Some(cell.len() as f64) }
                    })
                    .collect();
                if lengths.is_empty() {
                    return (0.0, 0.0);
                }
                let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
                let variance = lengths.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lengths.len() as f64;
                let stddev = variance.sqrt();
                (mean, stddev)
            })
            .collect();

        let meaningful: Vec<(f64, f64)> = col_stats.iter().copied().filter(|(m, _)| *m > 3.0).collect();

        if meaningful.len() >= 3 {
            let means: Vec<f64> = meaningful.iter().map(|(m, _)| *m).collect();
            let min_mean = means.iter().copied().fold(f64::INFINITY, f64::min);
            let max_mean = means.iter().copied().fold(0.0_f64, f64::max);

            let columns_uniform = min_mean > 0.0 && max_mean <= min_mean * 2.0;

            let low_variance = meaningful
                .iter()
                .all(|(mean, stddev)| *mean > 0.0 && *stddev / *mean < 0.3);

            if !skip_columnar_prose_guard && !dense_numeric_grid && columns_uniform && low_variance {
                return false;
            }
        }
    }

    if num_cols >= 3 {
        let mut unique_words: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for row in data_rows {
            for cell in row {
                for word in cell.split_whitespace() {
                    unique_words.insert(word);
                }
            }
        }
        let row_count = data_rows.len();
        if !dense_numeric_grid && row_count >= 3 && unique_words.len() < row_count * 2 {
            return false;
        }
    }

    if !grid.is_empty() {
        let header = &grid[0];
        let header_matches = data_rows
            .iter()
            .filter(|row| row.len() == header.len() && row.iter().zip(header.iter()).all(|(a, b)| a.trim() == b.trim()))
            .count();
        if header_matches >= 2 {
            return false;
        }
    }

    true
}

fn is_dense_numeric_grid(grid: &[Vec<String>]) -> bool {
    let Some(header) = grid.first() else {
        return false;
    };
    if header.len() < DENSE_NUMERIC_MIN_COLUMNS || grid.len() <= DENSE_NUMERIC_MIN_DATA_ROWS {
        return false;
    }

    let mut non_empty_cells = 0usize;
    let mut numeric_cells = 0usize;
    for cell in grid.iter().skip(1).flat_map(|row| row.iter()) {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        non_empty_cells += 1;
        if is_numeric_value_cell(trimmed) {
            numeric_cells += 1;
        }
    }

    non_empty_cells > 0
        && numeric_cells.saturating_mul(100) >= non_empty_cells.saturating_mul(DENSE_NUMERIC_MIN_CELL_PERCENT)
}

/// Whether the grid's data cells are overwhelmingly numeric values, with no
/// row/column-count floor (unlike [`is_dense_numeric_grid`], which is calibrated
/// for large 6×6+ tables). A short, wide grid of one-or-two-word cells that is
/// this numeric is genuine tabular data — a borderless invoice/line-item table —
/// not shredded prose, which is alphabetic. Used only to exempt such grids from
/// the ≥5-column single-word prose guard (xberg-io/xberg#1316).
fn is_predominantly_numeric_short_grid(grid: &[Vec<String>]) -> bool {
    // Measure the numeric fraction two ways and accept if either clears the bar:
    // over every data cell, and over the substantially-populated data rows only. A
    // borderless line-item table can carry a sparse continuation row — a wrapped
    // description with the remaining columns blank (xberg-io/xberg#1333). That row
    // is all-text and, pooled with the populated rows, drags the numeric fraction
    // below the bar. Requiring substantial rather than total occupancy also
    // tolerates a small number of inferred empty columns (xberg-io/xberg#1342).
    // This selective pass only grants the exemption, so it cannot demote a grid
    // the pooled pass already accepts.
    let width = grid.first().map_or(0, Vec::len);
    short_grid_numeric_ratio_meets_bar(grid, false) || (width > 0 && short_grid_numeric_ratio_meets_bar(grid, true))
}

/// Whether the numeric fraction of a short grid's data cells clears the
/// [`SHORT_NUMERIC_MIN_CELL_PERCENT`] bar with at least
/// [`SHORT_NUMERIC_MIN_DATA_CELLS`] cells of evidence. When
/// `substantially_populated_rows_only` is set, only rows meeting
/// [`SHORT_NUMERIC_MIN_ROW_OCCUPANCY_PERCENT`] contribute, so sparse continuation
/// rows and a few inferred empty columns do not distort the measurement (see
/// [`is_predominantly_numeric_short_grid`]).
fn short_grid_numeric_ratio_meets_bar(grid: &[Vec<String>], substantially_populated_rows_only: bool) -> bool {
    let width = grid.first().map_or(0, Vec::len);
    let mut non_empty_cells = 0usize;
    let mut numeric_cells = 0usize;
    for row in grid.iter().skip(1) {
        if substantially_populated_rows_only && !is_substantially_populated_data_row(row, width) {
            continue;
        }
        for cell in row {
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            non_empty_cells += 1;
            if is_numeric_value_cell(trimmed) {
                numeric_cells += 1;
            }
        }
    }
    non_empty_cells >= SHORT_NUMERIC_MIN_DATA_CELLS
        && numeric_cells.saturating_mul(100) >= non_empty_cells.saturating_mul(SHORT_NUMERIC_MIN_CELL_PERCENT)
}

/// Whether the row fills enough of the inferred grid to be self-contained.
fn is_substantially_populated_data_row(row: &[String], width: usize) -> bool {
    if width == 0 {
        return false;
    }
    let populated = row.iter().take(width).filter(|cell| !cell.trim().is_empty()).count();
    populated.saturating_mul(100) >= width.saturating_mul(SHORT_NUMERIC_MIN_ROW_OCCUPANCY_PERCENT)
}

fn is_dense_scalar_grid(grid: &[Vec<String>]) -> bool {
    let Some(header) = grid.first() else {
        return false;
    };
    let data_rows = grid.len().saturating_sub(1);
    if header.len() < DENSE_SCALAR_MIN_COLUMNS || data_rows < DENSE_SCALAR_MIN_DATA_ROWS {
        return false;
    }

    let total_cells = data_rows.saturating_mul(header.len());
    let mut filled_cells = 0usize;
    let mut compact_cells = 0usize;
    let mut digit_cells = 0usize;
    for cell in grid.iter().skip(1).flat_map(|row| row.iter()) {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        filled_cells += 1;
        if trimmed.chars().count() <= DENSE_SCALAR_MAX_CELL_CHARS && trimmed.split_whitespace().count() <= 2 {
            compact_cells += 1;
        }
        if trimmed.chars().any(|c| c.is_ascii_digit()) {
            digit_cells += 1;
        }
    }

    total_cells > 0
        && filled_cells.saturating_mul(100) >= total_cells.saturating_mul(DENSE_SCALAR_MIN_FILLED_PERCENT)
        && compact_cells.saturating_mul(100) >= filled_cells.saturating_mul(DENSE_SCALAR_MIN_COMPACT_PERCENT)
        && digit_cells.saturating_mul(100) >= filled_cells.saturating_mul(DENSE_SCALAR_MIN_DIGIT_PERCENT)
}

fn is_numeric_value_cell(cell: &str) -> bool {
    let digit_count = cell.chars().filter(char::is_ascii_digit).count();
    if digit_count == 0 {
        return false;
    }
    let alphanumeric_count = cell.chars().filter(|c| c.is_alphanumeric()).count();
    digit_count.saturating_mul(2) >= alphanumeric_count
}

/// Minimum fraction of non-empty table cells that must contain curly braces
/// (`{` or `}`) for the region to be classified as a code listing rather than
/// a table. At 0.20, one brace-containing cell per five non-empty cells is
/// enough to trigger the guard.
///
/// A separate hard-reject fires when any non-empty cell is *exactly* `{` or `}`:
/// isolated braces appear only in code block delimiters, never in real table data.
const CODE_BRACE_CELL_FRACTION: f64 = 0.20;

/// Returns `true` if the reconstructed table grid looks like a code listing
/// rather than genuine tabular data.
///
/// The layout model and text-edge heuristic occasionally misclassify code blocks
/// (especially C-family language listings with curly-brace syntax) as table
/// regions, because monospace character spacing creates apparent column positions.
///
/// Three signals are checked:
/// 1. **Hard reject**: any non-empty cell whose entire trimmed text is `{` or
///    `}` (an isolated brace cannot appear in real table content).
/// 2. **Fraction check**: if ≥ [`CODE_BRACE_CELL_FRACTION`] of non-empty cells
///    contain `{` or `}`, the region is likely code with inline block syntax.
/// 3. **Declaration grid**: a lone, unterminated C-family function declaration
///    head followed by pointer-bearing, comma-delimited parameter rows. A
///    terminal `);` or comma termination on every parameter row is required to
///    avoid rejecting API-reference tables with incidental code punctuation.
///
/// Python, Ruby, and other brace-free languages are not caught by this check;
/// those rarely produce false-positive tables at the heuristic tier.
pub(crate) fn looks_like_code_listing(table_cells: &[Vec<String>]) -> bool {
    let non_empty: Vec<&str> = table_cells
        .iter()
        .flat_map(|row| row.iter())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if non_empty.is_empty() {
        return false;
    }

    if non_empty.iter().any(|&cell| cell == "{" || cell == "}") {
        return true;
    }

    let brace_count = non_empty
        .iter()
        .filter(|&&cell| cell.contains('{') || cell.contains('}'))
        .count();
    (brace_count as f64) / (non_empty.len() as f64) >= CODE_BRACE_CELL_FRACTION
        || looks_like_declaration_grid(table_cells)
}

fn looks_like_declaration_grid(table_cells: &[Vec<String>]) -> bool {
    let Some(first_row) = table_cells.first() else {
        return false;
    };
    let mut first_cells = first_row.iter().map(|cell| cell.trim()).filter(|cell| !cell.is_empty());
    let Some(head) = first_cells.next() else {
        return false;
    };
    if first_cells.next().is_some() || !looks_like_declaration_head(head) {
        return false;
    }

    let continuation_rows: Vec<&[String]> = table_cells
        .iter()
        .skip(1)
        .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
        .map(Vec::as_slice)
        .collect();
    let evidence: Vec<ParameterRowEvidence> = continuation_rows
        .iter()
        .filter_map(|row| parameter_row_evidence(row))
        .collect();
    if evidence.len() < 2 || evidence.len() != continuation_rows.len() {
        return false;
    }

    let has_pointer = evidence.iter().any(|row| row.has_pointer);
    let has_closing_declaration = evidence.iter().any(|row| row.closes_declaration);
    let all_truncated_parameters = evidence.iter().all(|row| row.ends_with_comma);
    has_pointer && (has_closing_declaration || all_truncated_parameters)
}

#[derive(Clone, Copy)]
struct ParameterRowEvidence {
    ends_with_comma: bool,
    closes_declaration: bool,
    has_pointer: bool,
}

fn parameter_row_evidence(row: &[String]) -> Option<ParameterRowEvidence> {
    let cells: Vec<&str> = row
        .iter()
        .map(|cell| cell.trim())
        .filter(|cell| !cell.is_empty())
        .collect();
    if cells.len() < 2 {
        return None;
    }
    let last = cells.last()?;
    let (parameter_name, ends_with_comma, closes_declaration) = if let Some(name) = last.strip_suffix(',') {
        (name, true, false)
    } else if let Some(name) = last.strip_suffix(");") {
        (name, false, true)
    } else {
        return None;
    };
    if !looks_like_parameter_name(parameter_name) {
        return None;
    }

    Some(ParameterRowEvidence {
        ends_with_comma,
        closes_declaration,
        has_pointer: cells.iter().any(|cell| cell.contains('*')),
    })
}

fn looks_like_parameter_name(name: &str) -> bool {
    let name = name.trim().trim_start_matches('*');
    !name.is_empty()
        && name.chars().any(|character| character.is_alphabetic())
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '[' | ']'))
}

fn looks_like_declaration_head(head: &str) -> bool {
    let Some(prefix) = head.strip_suffix('(') else {
        return false;
    };
    let identifiers = prefix
        .split_whitespace()
        .filter(|token| token.chars().any(|character| character.is_alphabetic()))
        .count();
    identifiers >= 2
}

fn merge_header_only_column(table: &mut [Vec<String>], col: usize, header_text: String) {
    if table.is_empty() || table[0].is_empty() {
        return;
    }

    let trimmed = header_text.trim();
    if trimmed.is_empty() && table.len() > 1 {
        for row in table.iter_mut() {
            row.remove(col);
        }
        return;
    }

    if !trimmed.is_empty() {
        if col > 0 {
            let mut target = col - 1;
            while target > 0 && table[0][target].trim().is_empty() {
                target -= 1;
            }
            if !table[0][target].trim().is_empty() || target == 0 {
                if !table[0][target].is_empty() {
                    table[0][target].push(' ');
                }
                table[0][target].push_str(trimmed);
                for row in table.iter_mut() {
                    row.remove(col);
                }
                return;
            }
        }

        if col + 1 < table[0].len() {
            if table[0][col + 1].trim().is_empty() {
                table[0][col + 1] = trimmed.to_string();
            } else {
                let mut updated = trimmed.to_string();
                updated.push(' ');
                updated.push_str(table[0][col + 1].trim());
                table[0][col + 1] = updated;
            }
            for row in table.iter_mut() {
                row.remove(col);
            }
            return;
        }
    }

    for row in table.iter_mut() {
        row.remove(col);
    }
}

fn normalize_data_cell(cell: &mut String) {
    let mut text = cell.trim().to_string();
    if text.is_empty() {
        cell.clear();
        return;
    }

    for ch in ['\u{2014}', '\u{2013}', '\u{2212}'] {
        text = text.replace(ch, "-");
    }

    if text.starts_with("- ") {
        text = format!("-{}", text[2..].trim_start());
    }

    text = text.replace("- ", "-");
    text = text.replace(" -", "-");
    text = text.replace("E-", "e-").replace("E+", "e+");

    if text == "-" {
        text.clear();
    }

    *cell = text;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "pdf")]
    fn make_seg(text: &str, x: f32, y: f32, width: f32, height: f32) -> SegmentData {
        SegmentData {
            text: text.to_string(),
            x,
            y,
            width,
            height,
            font_size: height,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y: y,
            rotation_degrees: 0.0,
            assigned_role: None,
        }
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_split_single_word() {
        let seg = make_seg("Hello", 100.0, 500.0, 50.0, 12.0);
        let words = split_segment_to_words(&seg, 800.0);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[0].left, 100);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_split_two_words() {
        let seg = make_seg("Col A", 100.0, 500.0, 100.0, 12.0);
        let words = split_segment_to_words(&seg, 800.0);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Col");
        assert_eq!(words[1].text, "A");
        assert_eq!(words[1].left, 180);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_split_empty_segment() {
        let seg = make_seg("   ", 100.0, 500.0, 50.0, 12.0);
        let words = split_segment_to_words(&seg, 800.0);
        assert!(words.is_empty());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_split_many_words() {
        let seg = make_seg("a b c d", 0.0, 0.0, 700.0, 12.0);
        let words = split_segment_to_words(&seg, 800.0);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].text, "a");
        assert_eq!(words[1].text, "b");
        assert_eq!(words[2].text, "c");
        assert_eq!(words[3].text, "d");
        assert!(words[1].left > words[0].left);
        assert!(words[2].left > words[1].left);
        assert!(words[3].left > words[2].left);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_split_y_coordinate_conversion() {
        let seg = make_seg("word", 100.0, 500.0, 50.0, 12.0);
        let words = split_segment_to_words(&seg, 800.0);
        assert_eq!(words[0].top, 288);
        assert_eq!(words[0].height, 12);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn test_segments_to_words_multiple() {
        let segs = vec![
            make_seg("Hello", 10.0, 700.0, 40.0, 12.0),
            make_seg("World", 55.0, 700.0, 40.0, 12.0),
        ];
        let words = segments_to_words(&segs, 800.0);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[1].text, "World");
    }

    #[test]
    fn test_post_process_rejects_prose_as_table() {
        let table = vec![
            vec![
                "Foreword".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
                "ISO 21111-10:2021(E)".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "ISO".into(),
                "(the".into(),
                "International".into(),
                "Organization".into(),
                "for".into(),
                "Standardization)is".into(),
                "a".into(),
                "worldwide".into(),
            ],
            vec![
                "bodies".into(),
                "(ISO".into(),
                "member".into(),
                "bodies).The".into(),
                "work".into(),
                "of".into(),
                "preparing".into(),
                "International".into(),
            ],
            vec![
                "through".into(),
                "ISO".into(),
                "technical".into(),
                "committees.Each".into(),
                "member".into(),
                "body".into(),
                "interested".into(),
                "in".into(),
            ],
        ];
        let result = post_process_table(table, false, false);
        assert!(result.is_none(), "Prose-like table should be rejected");
    }

    #[test]
    fn test_post_process_accepts_real_table() {
        let table = vec![
            vec!["Name".into(), "Department".into(), "Annual Salary".into()],
            vec!["John Smith".into(), "Engineering Dept".into(), "$95,000".into()],
            vec!["Jane Doe".into(), "Marketing Team".into(), "$88,500".into()],
            vec!["Bob Johnson".into(), "Sales Division".into(), "$92,000".into()],
            vec!["Alice Williams".into(), "Human Resources".into(), "$85,000".into()],
        ];
        let result = post_process_table(table, false, false);
        assert!(result.is_some(), "Real table should be accepted");
    }

    #[test]
    fn dense_numeric_matrix_survives_anti_prose_guards() {
        let mut table = vec![
            (0..DENSE_NUMERIC_MIN_COLUMNS)
                .map(|col| format!("Column {col}"))
                .collect(),
        ];
        for row in 0..DENSE_NUMERIC_MIN_DATA_ROWS {
            table.push(
                (0..DENSE_NUMERIC_MIN_COLUMNS)
                    .map(|col| {
                        if col == 0 {
                            format!("{:03}", row)
                        } else {
                            "1.000".to_string()
                        }
                    })
                    .collect(),
            );
        }

        let processed = post_process_table(table, true, false).expect("dense numeric matrix should be retained");
        assert!(is_well_formed_table(&processed));
    }

    #[test]
    fn compact_numeric_boundary_does_not_bypass_anti_prose_guards() {
        for columns in [3, 5] {
            let mut table = vec![(0..columns).map(|col| format!("Column {col}")).collect()];
            table.extend((0..5).map(|_| vec!["1.000".to_string(); columns]));

            let accepted =
                post_process_table(table, true, false).is_some_and(|processed| is_well_formed_table(&processed));
            assert!(!accepted, "repetitive {columns}-column compact grid must be rejected");
        }
    }

    fn dense_grid_with_columns(columns: usize, rows: usize) -> Vec<Vec<String>> {
        let mut table = vec![(0..columns).map(|column| format!("Column {column}")).collect()];
        table.extend((0..rows).map(|row| (0..columns).map(|column| format!("{}.{column}", row + 1)).collect()));
        table
    }

    #[test]
    fn prunes_one_empty_header_interior_track_and_preserves_lone_text() {
        let mut table = dense_grid_with_columns(7, SPURIOUS_COLUMN_MIN_DATA_ROWS);
        table[0][3].clear();
        for row in table.iter_mut().skip(1) {
            row[3].clear();
        }
        *table.last_mut().expect("data row") = vec![
            "footer note".into(),
            "continues here".into(),
            "with text".into(),
            "sustained".into(),
            "near table".into(),
            "boundary words".into(),
            "end".into(),
        ];

        assert!(prune_spurious_interior_column(&mut table, true));
        assert_eq!(table[0].len(), 6);
        assert!(
            table
                .last()
                .expect("data row")
                .iter()
                .any(|cell| cell.contains("sustained"))
        );
    }

    #[test]
    fn preserves_legitimate_named_sparse_column() {
        let mut table = dense_grid_with_columns(7, SPURIOUS_COLUMN_MIN_DATA_ROWS);
        table[0][3] = "Optional flag".into();
        for row in table.iter_mut().skip(1) {
            row[3].clear();
        }
        table.last_mut().expect("data row")[3] = "Y".into();

        assert!(!prune_spurious_interior_column(&mut table, true));
        assert_eq!(table[0].len(), 7);
        assert_eq!(table[0][3], "Optional flag");
    }

    #[test]
    fn preserves_unnamed_sparse_column_populated_in_table_body() {
        let mut table = dense_grid_with_columns(7, SPURIOUS_COLUMN_MIN_DATA_ROWS);
        table[0][3].clear();
        for row in table.iter_mut().skip(1) {
            row[3].clear();
        }
        let middle = table.len() / 2;
        table[middle] = vec![
            "boundary note".into(),
            "continues here".into(),
            "with text".into(),
            "sustained".into(),
            "inside table".into(),
            "body words".into(),
            "end".into(),
        ];

        assert!(!prune_spurious_interior_column(&mut table, true));
        assert_eq!(table[0].len(), 7);
        assert_eq!(table[middle][3], "sustained");
    }

    #[test]
    fn preserves_multiple_sparse_interior_columns() {
        let mut table = dense_grid_with_columns(8, SPURIOUS_COLUMN_MIN_DATA_ROWS);
        for column in [2, 5] {
            table[0][column].clear();
            for row in table.iter_mut().skip(1) {
                row[column].clear();
            }
        }

        assert!(!prune_spurious_interior_column(&mut table, true));
        assert_eq!(table[0].len(), 8);
    }

    #[test]
    fn sparse_track_does_not_turn_prose_into_table() {
        let mut table = vec![vec![String::new(); 7]];
        table.extend((0..SPURIOUS_COLUMN_MIN_DATA_ROWS).map(|row| {
            vec![
                format!("section {row}"),
                format!("page {row}"),
                "quick".into(),
                String::new(),
                "brown".into(),
                "fox".into(),
                "continues".into(),
            ]
        }));

        let accepted = post_process_table(table, true, false).is_some_and(|processed| is_well_formed_table(&processed));
        assert!(!accepted);
    }

    #[test]
    fn repeated_row_shape_finds_three_numeric_fields_after_two_row_header() {
        let table = vec![
            vec![
                "Report 2024".into(),
                "Patient status".into(),
                "Metric 70".into(),
                "Treatment group".into(),
                "Metric 91".into(),
                "Final outcome".into(),
            ],
            vec![
                "".into(),
                "".into(),
                "(score)".into(),
                "".into(),
                "(years)".into(),
                "".into(),
            ],
            vec![
                "R1".into(),
                "active".into(),
                "4.36".into(),
                "A".into(),
                "52".into(),
                "SVR".into(),
            ],
            vec![
                "R2".into(),
                "active".into(),
                "6.37".into(),
                "B".into(),
                "35".into(),
                "SVR".into(),
            ],
            vec![
                "R3".into(),
                "active".into(),
                "7.84".into(),
                "A".into(),
                "46".into(),
                "SVR".into(),
            ],
        ];

        assert_eq!(find_data_start(&table, true), 2);
        assert_eq!(find_data_start(&table, false), 0);
    }

    #[test]
    fn categorical_subtotal_does_not_hide_leading_numeric_data_row() {
        let table = vec![
            vec![
                "R1".into(),
                "New York".into(),
                "4.36".into(),
                "needs review".into(),
                "52".into(),
                "SVR".into(),
            ],
            vec![
                "Subtotal for region".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "R2".into(),
                "active".into(),
                "6.37".into(),
                "B".into(),
                "35".into(),
                "SVR".into(),
            ],
            vec![
                "R3".into(),
                "active".into(),
                "7.84".into(),
                "A".into(),
                "46".into(),
                "SVR".into(),
            ],
            vec![
                "R4".into(),
                "active".into(),
                "5.12".into(),
                "B".into(),
                "41".into(),
                "SVR".into(),
            ],
        ];

        assert_eq!(find_data_start(&table, true), 0);
    }

    #[test]
    fn repeated_shape_does_not_skip_numeric_rows_without_header_gap() {
        let table = vec![
            vec!["1".into(), "2".into(), "3".into(), "".into(), "5".into(), "".into()],
            vec!["1".into(), "2".into(), "".into(), "4".into(), "5".into(), "".into()],
            vec!["1".into(), "2".into(), "3".into(), "4".into(), "".into(), "".into()],
            vec!["1".into(), "2".into(), "3".into(), "4".into(), "".into(), "".into()],
            vec!["1".into(), "2".into(), "3".into(), "4".into(), "".into(), "".into()],
        ];

        assert_eq!(find_data_start(&table, true), 0);
    }

    #[test]
    fn retains_large_scalar_table_with_numeric_multiline_header() {
        let mut table = vec![
            vec![
                "".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
                "Core amino".into(),
                "acid".into(),
                "".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "Patient".into(),
                "Genotype".into(),
                "Viral load".into(),
                "".into(),
                "Sex".into(),
                "Age".into(),
                "70".into(),
                "91".into(),
                "rs12979860".into(),
                "End of treatment".into(),
                "".into(),
            ],
            vec![
                "no".into(),
                "".into(),
                "(10 IU/ml) 6".into(),
                "".into(),
                "".into(),
                "(years)".into(),
                "".into(),
                "".into(),
                "".into(),
                "response".into(),
                "a".into(),
            ],
        ];
        for row in 1..=SPURIOUS_COLUMN_MIN_DATA_ROWS {
            table.push(vec![
                format!("R{row}"),
                "1a".into(),
                format!("{}.36", row + 3),
                String::new(),
                if row % 2 == 0 { "F".into() } else { "M".into() },
                format!("{}.6", row + 30),
                "R".into(),
                "C".into(),
                if row % 2 == 0 { "CT".into() } else { "CC".into() },
                "SVR".into(),
                String::new(),
            ]);
        }
        table.push(vec![
            "a SVR, sustained".into(),
            "virologic response;".into(),
            "non-SVR, no".into(),
            "sustained".into(),
            "virologic".into(),
            "response".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
        ]);

        let processed = post_process_table(table, true, false).expect("large scalar table should be retained");
        assert_eq!(processed[0].len(), 9);
        assert!(processed[0][0].contains("Patient"));
        assert!(is_well_formed_table(&processed));
    }

    #[test]
    fn test_column_text_flow_rejects_multicolumn_prose() {
        let table = vec![
            vec!["Header Left".into(), "Header Right".into()],
            vec![
                "The results of this experiment show that the proposed method".into(),
                "significantly outperforms the baseline in all metrics tested".into(),
            ],
            vec![
                "across multiple datasets including the standard benchmark".into(),
                "suite commonly used in the literature for evaluation of".into(),
            ],
            vec![
                "natural language processing tasks and related problems".into(),
                "involving text classification and information extraction".into(),
            ],
            vec![
                "methods that rely on deep learning architectures with".into(),
                "attention mechanisms and transformer-based embeddings".into(),
            ],
        ];
        let result_unsupervised = post_process_table(table.clone(), false, false);
        assert!(
            result_unsupervised.is_none(),
            "Multi-column prose should be rejected in unsupervised mode"
        );
        let result_guided = post_process_table(table, true, false);
        assert!(
            result_guided.is_none(),
            "Multi-column prose should be rejected in layout-guided mode"
        );
    }

    #[test]
    fn test_column_text_flow_accepts_real_two_column_table() {
        let table = vec![
            vec!["Feature".into(), "Description".into()],
            vec!["Authentication.".into(), "OAuth 2.0 with JWT tokens.".into()],
            vec!["Rate Limiting.".into(), "100 requests per minute.".into()],
            vec!["Caching.".into(), "Redis-backed with TTL.".into()],
            vec!["Monitoring.".into(), "Prometheus metrics endpoint.".into()],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_some(),
            "Real 2-column table with proper sentence endings should be accepted"
        );
    }

    #[test]
    fn test_column_text_flow_not_triggered_with_few_rows() {
        let table = vec![
            vec!["Left".into(), "Right".into()],
            vec![
                "some text without ending punct".into(),
                "continues here in lowercase".into(),
            ],
            vec!["another partial sentence".into(), "flowing into next column".into()],
        ];
        let _ = post_process_table(table, true, false);
    }

    #[test]
    fn test_layout_guided_rejects_prose_with_long_cells() {
        let long_cell = "a".repeat(120);
        let table = vec![
            vec!["Header A".into(), "Header B".into()],
            vec![long_cell.clone(), long_cell.clone()],
            vec![long_cell.clone(), long_cell.clone()],
            vec![long_cell.clone(), long_cell.clone()],
            vec![long_cell.clone(), long_cell.clone()],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_none(),
            "Layout-guided should reject tables with overwhelmingly long cells"
        );
    }

    #[test]
    fn test_layout_guided_accepts_table_with_some_long_cells() {
        let table = vec![
            vec!["Feature Name".into(), "Description".into()],
            vec![
                "User Authentication Module".into(),
                "Handles login, logout, and session management for users.".into(),
            ],
            vec![
                "Rate Limiting Service".into(),
                "Controls API request rates per client and endpoint.".into(),
            ],
            vec!["Cache Layer".into(), "Short desc.".into()],
            vec![
                "Monitoring Dashboard".into(),
                "Displays real-time metrics and alerting configuration.".into(),
            ],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_some(),
            "Layout-guided table with some long cells should be accepted"
        );
    }

    #[test]
    fn test_layout_guided_rejects_dominant_column() {
        let table = vec![
            vec!["Tag".into(), "Content".into()],
            vec!["x".into(), "This is a very long paragraph of text that contains almost all content in the table and dwarfs the tag column.".into()],
            vec!["y".into(), "Another massive block of text that makes the first column insignificant by comparison in terms of character count.".into()],
            vec!["z".into(), "Yet more extensive content that further skews the distribution of characters heavily toward this second column here.".into()],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_none(),
            "Layout-guided should reject tables with >92% text in one column"
        );
    }

    #[test]
    fn test_layout_guided_single_word_prose_rejected() {
        let table = vec![
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into(), "F".into()],
            vec![
                "The".into(),
                "quick".into(),
                "brown".into(),
                "fox".into(),
                "jumps".into(),
                "over".into(),
            ],
            vec![
                "the".into(),
                "lazy".into(),
                "dog".into(),
                "and".into(),
                "runs".into(),
                "away".into(),
            ],
            vec![
                "from".into(),
                "the".into(),
                "big".into(),
                "bad".into(),
                "wolf".into(),
                "today".into(),
            ],
            vec![
                "who".into(),
                "was".into(),
                "very".into(),
                "mean".into(),
                "and".into(),
                "scary".into(),
            ],
            vec![
                "but".into(),
                "the".into(),
                "fox".into(),
                "was".into(),
                "too".into(),
                "fast".into(),
            ],
            vec![
                "for".into(),
                "the".into(),
                "wolf".into(),
                "to".into(),
                "ever".into(),
                "catch".into(),
            ],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_none(),
            "Layout-guided should reject tables with >85% single-word cells"
        );
    }

    #[test]
    fn test_row_continuation_rejects_prose_flowing_across_rows() {
        let mut table = vec![vec!["Left Column".into(), "Right Column".into()]];
        let prose_pairs = vec![
            ("The experiment was conducted", "over several weeks and the"),
            ("results clearly demonstrate", "that the proposed method is"),
            ("superior to existing approaches", "because it leverages novel"),
            ("techniques developed in our", "laboratory during the past"),
            ("decade of intensive research", "on machine learning systems"),
        ];
        for (left, right) in prose_pairs {
            table.push(vec![left.into(), right.into()]);
        }
        let result = post_process_table(table.clone(), false, false);
        assert!(
            result.is_none(),
            "Row-continuation prose should be rejected in unsupervised mode"
        );
        let result_guided = post_process_table(table, true, false);
        assert!(
            result_guided.is_none(),
            "Row-continuation prose should be rejected in layout-guided mode"
        );
    }

    #[test]
    fn test_row_continuation_accepts_table_with_sentence_endings() {
        let table = vec![
            vec!["Parameter".into(), "Value".into()],
            vec!["Max connections.".into(), "100 per host.".into()],
            vec!["Timeout.".into(), "30 seconds.".into()],
            vec!["Retry policy.".into(), "Exponential backoff.".into()],
            vec!["Cache TTL.".into(), "3600 seconds.".into()],
            vec!["Rate limit.".into(), "1000 req/min.".into()],
        ];
        let result = post_process_table(table, true, false);
        assert!(
            result.is_some(),
            "Table with proper sentence endings should not be rejected by row-continuation check"
        );
    }

    #[test]
    fn test_high_row_low_column_rejects_prose() {
        let mut table = vec![vec!["Column A".into(), "Column B".into()]];
        for i in 0..25 {
            table.push(vec![
                format!("Content block {} left side text", i),
                format!("Content block {} right side text", i),
            ]);
        }
        let result = post_process_table(table.clone(), false, false);
        assert!(
            result.is_none(),
            "High-row low-column fully-filled table should be rejected (unsupervised)"
        );
        let result_guided = post_process_table(table, true, false);
        assert!(
            result_guided.is_none(),
            "High-row low-column fully-filled table should be rejected (layout-guided)"
        );
    }

    #[test]
    fn test_high_row_low_column_accepts_sparse_table() {
        let mut table = vec![vec!["Date".into(), "Event".into()]];
        for i in 0..25 {
            if i % 3 == 0 {
                table.push(vec![format!("2024-01-{:02}", i + 1), "Holiday.".into()]);
            } else {
                table.push(vec![format!("2024-01-{:02}", i + 1), String::new()]);
            }
        }
        let result = post_process_table(table, true, false);
        let _ = result;
    }

    #[test]
    fn test_high_row_low_column_allows_four_plus_columns() {
        let mut table = vec![vec!["ID".into(), "Name".into(), "Dept".into(), "Salary".into()]];
        for i in 0..25 {
            table.push(vec![
                format!("{}", i + 1),
                format!("Employee {}", i),
                "Engineering".into(),
                format!("${},000", 80 + i),
            ]);
        }
        let result = post_process_table(table, false, false);
        assert!(
            result.is_some(),
            "4-column table with many rows should not be rejected by high-row-low-column check"
        );
    }

    #[test]
    fn test_uniform_column_width_rejects_prose() {
        let mut table = vec![vec!["Col A".into(), "Col B".into(), "Col C".into()]];
        for _ in 0..8 {
            table.push(vec![
                "The quick brown fox jumps over".into(),
                "the lazy dog and runs through".into(),
                "the forest at remarkable speed".into(),
            ]);
        }
        let result = post_process_table(table.clone(), false, false);
        assert!(
            result.is_none(),
            "Uniform column width prose should be rejected (unsupervised)"
        );
        let result_guided = post_process_table(table, true, false);
        assert!(
            result_guided.is_none(),
            "Uniform column width prose should be rejected (layout-guided)"
        );
    }

    #[test]
    fn test_uniform_column_width_accepts_varied_columns() {
        let table = vec![
            vec!["ID".into(), "Product Name".into(), "Short Note".into()],
            vec![
                "1001".into(),
                "Industrial Premium Widget Alpha Series".into(),
                "High durability rating.".into(),
            ],
            vec![
                "1002".into(),
                "Advanced Sensor Gadget Beta Model".into(),
                "Wireless connectivity.".into(),
            ],
            vec![
                "1003".into(),
                "Professional Ergonomic Tool Gamma".into(),
                "Titanium blade.".into(),
            ],
            vec![
                "1004".into(),
                "Main Assembly Replacement Part Delta".into(),
                "Production line seven.".into(),
            ],
            vec![
                "1005".into(),
                "Standard Inventory Item Epsilon Unit".into(),
                "Daily operations use.".into(),
            ],
        ];
        let result = post_process_table(table, false, false);
        assert!(result.is_some(), "Table with varied column widths should be accepted");
    }

    #[test]
    fn test_well_formed_rejects_single_row() {
        let grid = vec![vec!["Header".into(), "Value".into()]];
        assert!(!is_well_formed_table(&grid), "Single-row grid should be rejected");
    }

    #[test]
    fn test_well_formed_rejects_single_column() {
        let grid = vec![vec!["Header".into()], vec!["Row 1".into()], vec!["Row 2".into()]];
        assert!(!is_well_formed_table(&grid), "Single-column grid should be rejected");
    }

    #[test]
    fn test_well_formed_accepts_real_table() {
        let grid = vec![
            vec!["Name".into(), "Department".into(), "Salary".into()],
            vec!["John Smith".into(), "Engineering".into(), "$95,000".into()],
            vec!["Jane Doe".into(), "Marketing".into(), "$88,500".into()],
            vec!["Bob Johnson".into(), "Sales".into(), "$92,000".into()],
            vec!["Alice Williams".into(), "HR".into(), "$85,000".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Real table with varied columns should be accepted"
        );
    }

    /// A genuine text-heavy key-value grid (#1319 invoice header) has regular,
    /// short column lengths, so the global uniform-column prose heuristic rejects
    /// it — but a geometrically pre-vetted caller passing
    /// `skip_columnar_prose_guard = true` must accept it while every other
    /// structural guard still applies.
    #[test]
    fn test_key_value_grid_gated_by_columnar_prose_guard_only() {
        let grid: Vec<Vec<String>> = vec![
            vec![
                "EXAMPLE COMPANY".into(),
                "Customer number".into(),
                "CUST-86241057".into(),
            ],
            vec![
                "Attn. SYNTH RECIPIENT".into(),
                "Invoice number".into(),
                "INV-709381624".into(),
            ],
            vec!["SAMPLE ROAD 14".into(), "Invoice date".into(), "15 January 2030".into()],
            vec!["45123 DEMO CITY".into(), "Order number".into(), "ORDER-58260419".into()],
            vec!["SYNTH COUNTRY".into(), "Order date".into(), "15 January 2030".into()],
            vec![
                "Tax ID SYNTH-TAX-918274635".into(),
                "Delivery date".into(),
                "15 January 2030".into(),
            ],
        ];
        assert!(
            !is_well_formed_table_core(&grid, false),
            "uniform-column prose heuristic rejects the key-value grid without the skip"
        );
        assert!(
            is_well_formed_table_core(&grid, true),
            "pre-vetted key-value grid must pass every other structural guard"
        );
    }

    #[test]
    fn test_well_formed_rejects_sparse_form_grid() {
        let grid: Vec<Vec<String>> = vec![
            vec!["".into(), "Tender".into(), "No.".into(), "".into()],
            vec!["41(01)/2019/PROM".into(), "".into(), "".into(), "".into()],
            vec!["Dated:".into(), "".into(), "11/09/2020".into(), "".into()],
            vec!["CPP".into(), "Portal".into(), "Tender".into(), "ID:".into()],
            vec!["2020_TBI_582964_1".into(), "".into(), "".into(), "".into()],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Sparse form-like grid (>40% empty cells) should be rejected"
        );
    }

    #[test]
    fn test_well_formed_rejects_repetitive_content() {
        let grid = vec![
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Repetitive content (same words every row) should be rejected"
        );
    }

    #[test]
    fn test_well_formed_rejects_repeated_header_in_data() {
        let grid = vec![
            vec!["Title".into(), "Author".into(), "Page".into()],
            vec!["Chapter 1".into(), "Smith".into(), "10".into()],
            vec!["Title".into(), "Author".into(), "Page".into()],
            vec!["Chapter 2".into(), "Doe".into(), "25".into()],
            vec!["Title".into(), "Author".into(), "Page".into()],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Table with header repeated in data rows should be rejected"
        );
    }

    #[test]
    fn test_well_formed_rejects_prose_rows() {
        let grid = vec![
            vec!["Column A".into(), "Column B".into(), "Column C".into()],
            vec![
                "The experiment was conducted over".into(),
                "several weeks and the results clearly".into(),
                "demonstrate that the proposed method is".into(),
            ],
            vec![
                "superior to existing approaches because".into(),
                "it leverages novel techniques developed".into(),
                "in our laboratory during the past decade".into(),
            ],
            vec![
                "of intensive research on machine learning".into(),
                "systems and their applications to natural".into(),
                "language processing and text extraction".into(),
            ],
            vec![
                "from documents in various formats including".into(),
                "portable document format and hypertext markup".into(),
                "language as well as office document formats".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Multi-column prose should be rejected by row coherence check"
        );
    }

    #[test]
    fn test_well_formed_rejects_uniform_columns() {
        let grid = vec![
            vec!["Col A".into(), "Col B".into(), "Col C".into()],
            vec!["twelve chars".into(), "twelve char2".into(), "twelve char3".into()],
            vec!["twelve char4".into(), "twelve char5".into(), "twelve char6".into()],
            vec!["twelve char7".into(), "twelve char8".into(), "twelve char9".into()],
            vec!["twelve charA".into(), "twelve charB".into(), "twelve charC".into()],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Table with uniform column widths and low variance should be rejected"
        );
    }

    #[test]
    fn test_well_formed_accepts_varied_columns() {
        let grid = vec![
            vec!["ID".into(), "Product Name".into(), "Price".into()],
            vec!["1".into(), "Widget Alpha Premium".into(), "$29.99".into()],
            vec!["2".into(), "Gadget Beta Standard".into(), "$149.50".into()],
            vec!["3".into(), "Tool Gamma Deluxe Ed".into(), "$7.25".into()],
            vec!["4".into(), "Part Delta Industrial".into(), "$1,299.00".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Table with varied column types should be accepted"
        );
    }

    #[test]
    fn test_well_formed_rejects_multicolumn_prose_short_cells() {
        let grid = vec![
            vec!["Bookmark".into(), "File PDF".into(), "Year 4".into()],
            vec!["Numeracy".into(), "Essment".into(), "Test".into()],
            vec![
                "Papers is universally".into(),
                "And Answers compatible".into(),
                "with any".into(),
            ],
            vec!["devices".into(), "to read".into(), "".into()],
            vec!["Year 4 Maths".into(), "Lesson".into(), "Uk The".into()],
            vec!["Maths Guy".into(), "ninety fail".into(), "Can you".into()],
            vec!["pass a GRADE".into(), "four Math".into(), "Test here".into()],
            vec!["Quick Learnerz".into(), "Year".into(), "four Termly".into()],
            vec!["Maths Assessment".into(), "Can".into(), "You Pass".into()],
            vec!["".into(), "Page five".into(), "".into()],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "3-column prose with short cells (nougat_008 pattern) should be rejected"
        );
    }

    #[test]
    fn test_well_formed_rejects_two_row_columned_prose() {
        let grid = vec![
            vec!["Column A".into(), "Column B".into(), "Column C".into()],
            vec![
                "The experiment was conducted over".into(),
                "several weeks and the results clearly".into(),
                "demonstrate that the proposed method is".into(),
            ],
            vec![
                "superior to existing approaches because".into(),
                "it leverages novel techniques developed".into(),
                "in our laboratory during the past decade".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Two-row column-aligned prose should be demoted (issue #36)"
        );
    }

    #[test]
    fn test_well_formed_rejects_two_col_two_row_prose() {
        let grid = vec![
            vec!["Column A".into(), "Column B".into()],
            vec![
                "The experiment was conducted over".into(),
                "several weeks and the results clearly".into(),
            ],
            vec![
                "demonstrate that the proposed method".into(),
                "is superior to existing approaches here".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Two-column, two-row prose should be demoted (issue #36)"
        );
    }

    #[test]
    fn test_well_formed_rejects_single_data_row_prose() {
        let grid = vec![
            vec!["Column A".into(), "Column B".into(), "Column C".into()],
            vec![
                "The experiment was conducted over".into(),
                "several weeks and the results clearly".into(),
                "demonstrate that the proposed method is".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Single-data-row column-aligned prose should be demoted (issue #36)"
        );
    }

    #[test]
    fn test_well_formed_rejects_five_col_short_prose() {
        let grid = vec![
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
            vec![
                "conducted over several weeks".into(),
                "and the results clearly show".into(),
                "that the proposed method here".into(),
                "is superior to existing work".into(),
                "because of novel techniques used".into(),
            ],
            vec![
                "developed in our laboratory over".into(),
                "the past decade of intensive".into(),
                "research on machine learning here".into(),
                "and its applications to natural".into(),
                "language processing of documents".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Five-column short prose (upper boundary) should be demoted (issue #36)"
        );
    }

    #[test]
    fn test_sparse_continuation_row_keeps_numeric_line_item_table() {
        // A five-column borderless line-item table whose second row is a wrapped
        // description continuation (trailing columns blank). Pooled over all
        // cells the numeric fraction is 4/8 = 50%, below the 60% bar, but the
        // complete item row alone is 4/5 = 80% numeric. The continuation row
        // must not erase the table (issue #1333).
        let table = vec![
            vec![
                "Item".into(),
                "Qty".into(),
                "Price".into(),
                "VAT".into(),
                "Total".into(),
            ],
            vec![
                "SYNTH PRODUCT".into(),
                "1".into(),
                "120.40".into(),
                "19%".into(),
                "120.40".into(),
            ],
            vec![
                "WITH FEE".into(),
                "each".into(),
                "$".into(),
                String::new(),
                String::new(),
            ],
        ];
        let result = post_process_table(table.clone(), true, false);
        assert!(
            result.is_some(),
            "Numeric line-item table with a sparse continuation row must survive (layout-guided)"
        );
        let result_unsupervised = post_process_table(table, false, false);
        assert!(
            result_unsupervised.is_some(),
            "Numeric line-item table with a sparse continuation row must survive (unsupervised)"
        );
    }

    #[test]
    fn test_inferred_columns_keep_sparse_numeric_line_item_table() {
        // The visible table has five columns, but reconstruction can infer two
        // extra tracks. The principal row still supplies 6/7 occupied cells and
        // four numeric values; the sparse fee row must not dilute that evidence.
        let table = vec![
            vec![
                "Item".into(),
                "Quantity".into(),
                "Price".into(),
                "VAT".into(),
                "Total".into(),
                String::new(),
                String::new(),
            ],
            vec![
                "SYNTH PRODUCT".into(),
                "1".into(),
                "120.40".into(),
                "19%".into(),
                "120.40".into(),
                "split".into(),
                String::new(),
            ],
            vec![
                "INCLUDING SYNTHETIC DEVICE FEE".into(),
                "1".into(),
                "3.40".into(),
                String::new(),
                String::new(),
                "split".into(),
                "tail".into(),
            ],
        ];

        assert!(
            post_process_table(table.clone(), true, false).is_some(),
            "numeric line-item table must survive a small inferred-column overrun"
        );
        assert!(
            post_process_table(table, false, false).is_some(),
            "the inferred-column recovery must not depend on layout guidance"
        );
    }

    #[test]
    fn test_sparse_numeric_prose_does_not_bypass_short_grid_guard() {
        let table = vec![
            vec![
                "A".into(),
                "B".into(),
                "C".into(),
                "D".into(),
                "E".into(),
                "F".into(),
                "G".into(),
            ],
            vec![
                "alpha".into(),
                "1".into(),
                "2".into(),
                "3".into(),
                String::new(),
                String::new(),
                String::new(),
            ],
            vec![
                "beta".into(),
                String::new(),
                String::new(),
                String::new(),
                "x".into(),
                "4".into(),
                "tail".into(),
            ],
        ];

        assert!(
            post_process_table(table.clone(), true, false).is_none(),
            "sparse numeric prose must remain rejected when no row fills 85% of the inferred grid"
        );
        assert!(
            post_process_table(table, false, false).is_none(),
            "the issue #36 guard must remain active without layout guidance"
        );
    }

    #[test]
    fn test_well_formed_rejects_short_wide_sparse_contact_block() {
        let grid = vec![
            vec![
                String::new(),
                "30B5".into(),
                "Stevenson".into(),
                "Drive·".into(),
                "Suite".into(),
                "301. Springfield,".into(),
                String::new(),
                "IL 62703".into(),
            ],
            vec![
                "Telephone".into(),
                String::new(),
                "(217)".into(),
                "585-2370'".into(),
                "(888)".into(),
                "547-8473·".into(),
                "Fax (217)".into(),
                "585-2372".into(),
            ],
            vec![
                String::new(),
                "a-mail:".into(),
                String::new(),
                "suaa@suaa.org·website:WWN.su88.oro".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
        ];

        assert!(
            !is_well_formed_table(&grid),
            "a sparse three-line contact block must not be promoted to a table"
        );
    }

    #[test]
    fn test_well_formed_keeps_two_row_numeric_table() {
        let grid = vec![
            vec!["Q1".into(), "Q2".into(), "Q3".into()],
            vec!["12".into(), "8".into(), "20".into()],
            vec!["15".into(), "9".into(), "24".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Two-row numeric table must survive the short-prose guard"
        );
    }

    #[test]
    fn test_well_formed_keeps_key_value_numeric() {
        let grid = vec![
            vec!["Metric".into(), "Value".into()],
            vec!["Total".into(), "$1,299.00".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Key/value pair with a numeric value must survive the short-prose guard"
        );
    }

    #[test]
    fn test_well_formed_keeps_unit_rows() {
        let grid = vec![
            vec!["Property".into(), "Measurement".into()],
            vec!["Length".into(), "45 mm".into()],
            vec!["Voltage".into(), "3.3 V".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Unit-bearing rows must survive the short-prose guard (digit-bearing exemption)"
        );
    }

    #[test]
    fn test_well_formed_keeps_short_label_key_value() {
        let grid = vec![
            vec!["Field".into(), "Entry".into()],
            vec!["Status".into(), "Active".into()],
            vec!["Country".into(), "France".into()],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Short-label key/value (< 4 words/cell) must survive the short-prose guard"
        );
    }

    #[test]
    fn test_well_formed_rejects_wide_two_row_shredded_prose() {
        let grid = vec![
            vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into(), "F".into()],
            vec![
                "the above equation by".into(),
                "the factor applied to".into(),
                "the initial density field".into(),
                "yields a cloud radius".into(),
                "of roughly ten to the".into(),
                "seventeen centimeters here".into(),
            ],
            vec![
                "which is approximately equal".into(),
                "to point zero three parsec".into(),
                "measured for all of the".into(),
                "models considered throughout".into(),
                "the present numerical study".into(),
                "of collapsing molecular clouds".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Wide two-row phrase-per-cell prose should be demoted (issue #36, wide variant)"
        );
    }

    #[test]
    fn test_well_formed_rejects_wide_shredded_prose_with_incidental_numbers() {
        let grid = vec![
            vec![
                "oblate range".into(),
                "clouds".into(),
                "have are used".into(),
                "ra = to add".into(),
                "rb = noise".into(),
                "R and to".into(),
                "rc = initial".into(),
                "density".into(),
                "R . Random".into(),
                "distributions".into(),
                "numbers".into(),
                "by multiplying".into(),
                "( x, y,".into(),
                "z )) in".into(),
                "the from".into(),
            ],
            vec![
                "the".into(),
                "above".into(),
                "equation".into(),
                "by".into(),
                "the factor".into(),
                "0 . 1 ran".into(),
                "( x, y,".into(),
                "z )].".into(),
                "The".into(),
                "cloud radius".into(),
                "is".into(),
                "R =".into(),
                "1 . 0".into(),
                "10 17".into(),
                "cm".into(),
            ],
        ];
        assert!(
            !is_well_formed_table(&grid),
            "Wide shredded prose with incidental numbers should be demoted (issue #36)"
        );
    }

    #[test]
    fn test_well_formed_keeps_wide_short_value_grid() {
        let grid = vec![
            vec![
                "Q1".into(),
                "Q2".into(),
                "Q3".into(),
                "Q4".into(),
                "FY".into(),
                "YoY".into(),
            ],
            vec![
                "12".into(),
                "8".into(),
                "20".into(),
                "15".into(),
                "55".into(),
                "+4%".into(),
            ],
            vec![
                "14".into(),
                "9".into(),
                "22".into(),
                "17".into(),
                "62".into(),
                "+7%".into(),
            ],
        ];
        assert!(
            is_well_formed_table(&grid),
            "Wide numeric short-value grid must survive the widened short-prose guard"
        );
    }

    #[test]
    fn test_looks_like_short_columned_prose_signal() {
        let prose = vec![
            vec![
                "The experiment was conducted over".into(),
                "several weeks and the results clearly".into(),
                "demonstrate that the proposed method is".into(),
            ],
            vec![
                "superior to existing approaches because".into(),
                "it leverages novel techniques developed".into(),
                "in our laboratory during the past decade".into(),
            ],
        ];
        assert!(
            looks_like_short_columned_prose(&prose, 3),
            "phrase-per-cell prose rows read as prose"
        );

        let numeric = vec![vec!["12".into(), "8".into(), "20".into()]];
        assert!(!looks_like_short_columned_prose(&numeric, 3), "numeric rows are exempt");

        let short_labels = vec![vec!["Status".into(), "Active".into()]];
        assert!(
            !looks_like_short_columned_prose(&short_labels, 2),
            "short-label rows (< 4 words/cell) are not prose"
        );
    }

    #[test]
    fn declaration_shaped_code_grids_are_rejected() {
        let fill_string = vec![
            vec!["void FillString(".into(), "".into()],
            vec!["TCHAR*".into(), "buf,".into()],
            vec!["size_t".into(), "cchBuf,".into()],
        ];
        let get_file_version = vec![
            vec!["BOOL GetFileVersion(".into(), "".into(), "".into()],
            vec!["LPCWSTR".into(), "lpsFile,".into(), "".into()],
            vec!["__out".into(), "FILE_VERSION".into(), "*pVersion);".into()],
        ];
        let encode_stream = vec![
            vec!["size_t EncodeStream(".into(), "".into(), "".into()],
            vec!["__in".into(), "HANDLE".into(), "hStream,".into()],
            vec!["__inout".into(), "STREAM".into(), "*pStream);".into()],
        ];

        for grid in [&fill_string, &get_file_version, &encode_stream] {
            assert!(looks_like_code_listing(grid));
        }
    }

    #[test]
    fn api_reference_grid_with_code_punctuation_is_not_rejected() {
        let grid = vec![
            vec!["Function".into(), "Signature".into(), "Description".into()],
            vec![
                "allocate()".into(),
                "void* allocate(size_t);".into(),
                "Allocates a buffer, or returns null".into(),
            ],
            vec![
                "release(ptr)".into(),
                "void release(void*);".into(),
                "Releases the supplied buffer".into(),
            ],
        ];

        assert!(!looks_like_code_listing(&grid));
    }

    #[test]
    fn merged_api_title_and_parameter_descriptions_are_not_rejected() {
        let grid = vec![
            vec!["Function Parameters (".into(), "".into(), "".into()],
            vec!["Type".into(), "Name".into(), "Description".into()],
            vec![
                "char *".into(),
                "buffer".into(),
                "Destination pointer, must be writable".into(),
            ],
            vec!["size_t".into(), "length".into(), "Bytes, excluding terminator;".into()],
        ];

        assert!(!looks_like_code_listing(&grid));
    }

    #[test]
    fn required_field_pointer_footnote_is_not_rejected() {
        let grid = vec![
            vec!["Required Fields (".into(), "".into()],
            vec!["Name*".into(), "Primary contact,".into()],
            vec!["Owner".into(), "Responsible team,".into()],
            vec!["".into(), "* Required field".into()],
        ];

        assert!(!looks_like_code_listing(&grid));
    }

    #[test]
    fn post_processed_declaration_grid_is_rejected_as_code() {
        let grid = vec![
            vec!["BOOL GetFileVersion(".into(), "".into(), "".into()],
            vec!["LPCWSTR".into(), "lpsFile,".into(), "".into()],
            vec!["__out".into(), "FILE_VERSION".into(), "*pVersion);".into()],
        ];
        let cleaned = post_process_table(grid, true, false).expect("declaration grid should survive table cleanup");

        assert!(looks_like_code_listing(&cleaned));
    }

    #[test]
    fn numeric_grid_is_not_rejected_as_code() {
        let grid = vec![
            vec!["Year".into(), "Revenue".into(), "Margin".into()],
            vec!["2024".into(), "1,250".into(), "18.5%".into()],
            vec!["2025".into(), "1,420".into(), "20.1%".into()],
        ];

        assert!(!looks_like_code_listing(&grid));
    }

    /// Regression test for xberg-io/xberg#1301 (mode b): a colon-introduced,
    /// semicolon-delimited 2-item list whose clauses were word-per-cell
    /// reconstructed into a 10-column, 2-data-row grid. The existing
    /// row-coherence guards all require >= 3 or >= 4 data rows and never fire
    /// on this shape; `looks_like_shredded_prose_row` must reject it directly.
    #[test]
    fn short_word_shredded_prose_grid_is_rejected() {
        let grid = vec![
            vec![
                "to exclude".into(),
                "fractional".into(),
                "amounts".into(),
                "from the".into(),
                "shareholders'".into(),
                "".into(),
                "subscription".into(),
                "right;".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "where".into(),
                "the new shares".into(),
                "are issued".into(),
                "against".into(),
                "cash".into(),
                "contributions".into(),
                "".into(),
                "at market price;".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "where".into(),
                "the capital".into(),
                "is increased".into(),
                "against".into(),
                "contributions".into(),
                "".into(),
                "in kind".into(),
                "for the purpose".into(),
                "of merging".into(),
                "companies;".into(),
            ],
        ];

        assert!(
            !is_well_formed_table(&grid),
            "short word-shredded prose run must be rejected as a table"
        );
    }

    /// A single short-row prose fragment (1 data row) must also be caught —
    /// the guard must not require >= 2 data rows either.
    #[test]
    fn single_row_word_shredded_prose_grid_is_rejected() {
        let grid = vec![
            vec![
                "to exclude".into(),
                "fractional".into(),
                "amounts".into(),
                "from the".into(),
                "shareholders'".into(),
                "".into(),
                "subscription".into(),
                "right;".into(),
                "".into(),
                "".into(),
            ],
            vec![
                "where".into(),
                "the new shares".into(),
                "are issued".into(),
                "against".into(),
                "cash".into(),
                "contributions".into(),
                "".into(),
                "at market price;".into(),
                "".into(),
                "".into(),
            ],
        ];

        assert!(!is_well_formed_table(&grid));
    }

    /// A short, wide, but genuinely tabular grid (sparse per-row fill, no
    /// clause-terminal punctuation) must survive: the guard is scoped to
    /// dense, sentence-shaped rows, not merely "few rows and many columns".
    #[test]
    fn short_wide_sparse_numeric_grid_is_not_rejected_as_shredded_prose() {
        let grid = vec![
            vec![
                "NAME".into(),
                "ADDRESS".into(),
                "PCT".into(),
                "CLASS".into(),
                "COMMIT".into(),
                "TOTAL".into(),
            ],
            vec![
                "Northern Pension Trust".into(),
                "1 Lake Road, Zurich".into(),
                "15.20%".into(),
                "Limited Partner".into(),
                "45,040,000.00".into(),
                "45,233,052.00".into(),
            ],
        ];

        assert!(
            is_well_formed_table(&grid),
            "a real numeric/name table row must not be mistaken for shredded prose"
        );
        assert!(!looks_like_shredded_prose_row(&grid[1], grid[0].len()));
    }

    /// Single-word `HocrWord` at a given position, for the #1399 geometry tests.
    fn geometry_word(text: &str, left: u32, top: u32, width: u32) -> HocrWord {
        HocrWord {
            text: text.to_string(),
            left,
            top,
            width,
            height: 20,
            confidence: 95.0,
        }
    }

    /// GH#1399: prose whose lines run across the inferred column boundaries on
    /// every row must score as almost entirely straddled. Three rows of words
    /// each spanning both boundaries — the shape the reported page has.
    #[test]
    fn straddled_ratio_is_near_total_for_prose_running_across_every_boundary() {
        let mut region = Vec::new();
        for (row, top) in [0u32, 40, 80].iter().enumerate() {
            // Each word starts inside one column and ends inside the next.
            region.push(geometry_word(&format!("a{row}"), 0, *top, 130));
            region.push(geometry_word(&format!("b{row}"), 140, *top, 130));
        }
        let columns = vec![0u32, 100, 200];

        let ratio = straddled_boundary_ratio(&region, &columns);
        assert_eq!(
            ratio, 1.0,
            "every boundary is crossed on every row, so the ratio must be exactly 1.0; got {ratio}"
        );
        assert!(
            !is_well_formed_borderless_table(
                &[
                    vec!["a0".to_string(), "b0".to_string(), String::new()],
                    vec!["a1".to_string(), "b1".to_string(), String::new()],
                ],
                &region,
                &columns,
                0,
            ),
            "a rule-less region straddled on every row must be rejected as prose"
        );
    }

    /// The false-positive guard. A legitimate table whose first column holds
    /// one long word must NOT be rejected. An earlier attempt at this gate put
    /// the boundary at the midpoint between two column *medians* and used
    /// `any()` rather than a per-row proportion, so this exact shape — one wide
    /// word, every other cell clean — was misread as bridging.
    #[test]
    fn long_word_in_a_wide_column_does_not_read_as_a_straddled_boundary() {
        let region = vec![
            geometry_word("Department", 0, 0, 80),
            geometry_word("Head", 200, 0, 30),
            geometry_word("Telecommunications", 0, 40, 150),
            geometry_word("Alice", 200, 40, 40),
            geometry_word("Finance", 0, 80, 60),
            geometry_word("Bob", 200, 80, 30),
        ];
        let columns = vec![0u32, 200];

        let ratio = straddled_boundary_ratio(&region, &columns);
        assert_eq!(
            ratio, 0.0,
            "no word reaches column 1's start at x=200, so nothing straddles; got {ratio}"
        );
    }

    /// Signal 1 outranks Signal 2: a producer that drew ruling lines gets the
    /// benefit of the doubt even when the geometry looks prose-like, because
    /// the geometric signal alone is too weak to overrule drawn structure.
    #[test]
    fn drawn_ruling_lines_admit_a_region_the_geometric_gate_would_reject() {
        let mut region = Vec::new();
        for (row, top) in [0u32, 40, 80].iter().enumerate() {
            region.push(geometry_word(&format!("a{row}"), 0, *top, 130));
            region.push(geometry_word(&format!("b{row}"), 140, *top, 130));
        }
        let columns = vec![0u32, 100, 200];
        let grid = vec![
            vec!["Name".to_string(), "Role".to_string()],
            vec!["Alice".to_string(), "Engineer".to_string()],
            vec!["Bob".to_string(), "Designer".to_string()],
        ];

        assert_eq!(
            straddled_boundary_ratio(&region, &columns),
            1.0,
            "precondition: this geometry is fully straddled"
        );
        assert!(
            is_well_formed_borderless_table(&grid, &region, &columns, 3),
            "3 horizontal rules must admit the candidate despite the straddled geometry"
        );
        assert!(
            !is_well_formed_borderless_table(&grid, &region, &columns, 0),
            "the same candidate with no rules must fall through to the geometric gate and be rejected"
        );
    }

    /// Fewer than two detected columns means there is no boundary to straddle.
    #[test]
    fn straddled_ratio_is_zero_when_there_is_no_column_boundary() {
        let region = vec![geometry_word("only", 0, 0, 50)];
        assert_eq!(straddled_boundary_ratio(&region, &[0]), 0.0);
        assert_eq!(straddled_boundary_ratio(&region, &[]), 0.0);
        assert_eq!(straddled_boundary_ratio(&[], &[0, 100]), 0.0);
    }
}
