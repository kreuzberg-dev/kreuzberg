//! Core table reconstruction types and algorithms.
//!
//! This module provides the `HocrWord` type and table reconstruction functions
//! that are shared between the OCR and PDF modules. The algorithms detect
//! column/row structure from word bounding boxes and reconstruct tabular layouts.
//!
//! Originally adapted from the `hocr` module of `html-to-markdown-rs` (removed in v3).

/// Represents a word extracted from hOCR (or any source) with position and confidence information.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct HocrWord {
    /// Recognized word text.
    pub text: String,
    /// Left edge of the word bounding box in pixels.
    pub left: u32,
    /// Top edge of the word bounding box in pixels.
    pub top: u32,
    /// Bounding box width in pixels.
    pub width: u32,
    /// Bounding box height in pixels.
    pub height: u32,
    /// OCR confidence score (0.0–100.0).
    pub confidence: f64,
}

impl HocrWord {
    /// Get the right edge position.
    #[cfg(test)]
    #[inline]
    pub(crate) fn right(&self) -> u32 {
        self.left + self.width
    }

    /// Get the bottom edge position.
    #[cfg(test)]
    #[inline]
    pub(crate) fn bottom(&self) -> u32 {
        self.top + self.height
    }

    /// Get the vertical center position.
    #[inline]
    pub(crate) fn y_center(&self) -> f64 {
        self.top as f64 + (self.height as f64 / 2.0)
    }

    /// Get the horizontal center position.
    #[cfg(test)]
    #[inline]
    pub(crate) fn x_center(&self) -> f64 {
        self.left as f64 + (self.width as f64 / 2.0)
    }
}

/// Detect column positions from word x-coordinates.
///
/// Groups words by approximate x-position (within `column_threshold` pixels)
/// and returns the median x-position for each detected column, sorted left to right.
pub(crate) fn detect_columns(words: &[HocrWord], column_threshold: u32) -> Vec<u32> {
    if words.is_empty() {
        return Vec::new();
    }

    let mut position_groups: Vec<Vec<u32>> = Vec::new();

    for word in words {
        let x_pos = word.left;

        let mut found_group = false;
        for group in &mut position_groups {
            if let Some(&first_pos) = group.first()
                && x_pos.abs_diff(first_pos) <= column_threshold
            {
                group.push(x_pos);
                found_group = true;
                break;
            }
        }

        if !found_group {
            position_groups.push(vec![x_pos]);
        }
    }

    let mut columns: Vec<u32> = position_groups
        .iter()
        .filter(|group| !group.is_empty())
        .map(|group| {
            let mut sorted = group.clone();
            sorted.sort_unstable();
            let mid = sorted.len() / 2;
            sorted[mid]
        })
        .collect();

    columns.sort_unstable();
    columns
}

/// Detect row positions from word y-coordinates.
///
/// Groups words by their vertical center position and returns the median
/// y-position for each detected row. The `row_threshold_ratio` is multiplied
/// by the median word height to determine the grouping threshold.
pub(crate) fn detect_rows(words: &[HocrWord], row_threshold_ratio: f64) -> Vec<u32> {
    if words.is_empty() {
        return Vec::new();
    }

    let mut heights: Vec<u32> = words.iter().map(|w| w.height).collect();
    heights.sort_unstable();
    let median_height = heights[heights.len() / 2];
    let row_threshold = (median_height as f64 * row_threshold_ratio) as u32;

    let mut position_groups: Vec<Vec<f64>> = Vec::new();

    for word in words {
        let y_center = word.y_center();

        let mut found_group = false;
        for group in &mut position_groups {
            if let Some(&first_pos) = group.first()
                && (y_center - first_pos).abs() <= row_threshold as f64
            {
                group.push(y_center);
                found_group = true;
                break;
            }
        }

        if !found_group {
            position_groups.push(vec![y_center]);
        }
    }

    let mut rows: Vec<u32> = position_groups
        .iter()
        .filter(|group| !group.is_empty())
        .map(|group| {
            let mut sorted = group.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let mid = sorted.len() / 2;
            sorted[mid] as u32
        })
        .collect();

    rows.sort_unstable();
    rows
}

/// Find which row a word belongs to based on its y-center.
fn find_row_index(row_positions: &[u32], word: &HocrWord) -> Option<usize> {
    let y_center = word.y_center() as u32;

    row_positions
        .iter()
        .enumerate()
        .min_by_key(|&(_, row_y)| row_y.abs_diff(y_center))
        .map(|(idx, _)| idx)
}

/// Find which column a word belongs to based on its x-position.
fn find_column_index(col_positions: &[u32], word: &HocrWord) -> Option<usize> {
    let x_pos = word.left;

    col_positions
        .iter()
        .enumerate()
        .min_by_key(|&(_, col_x)| col_x.abs_diff(x_pos))
        .map(|(idx, _)| idx)
}

/// Remove empty rows and columns from a table grid.
fn remove_empty_rows_and_columns(table: Vec<Vec<String>>) -> Vec<Vec<String>> {
    if table.is_empty() {
        return table;
    }

    let num_cols = table[0].len();
    let mut non_empty_cols: Vec<bool> = vec![false; num_cols];

    for row in &table {
        for (col_idx, cell) in row.iter().enumerate() {
            if !cell.trim().is_empty() {
                non_empty_cols[col_idx] = true;
            }
        }
    }

    table
        .into_iter()
        .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
        .map(|row| {
            row.into_iter()
                .enumerate()
                .filter(|(idx, _)| non_empty_cols[*idx])
                .map(|(_, cell)| cell)
                .collect()
        })
        .collect()
}

/// Reconstruct a table grid from words with bounding box positions.
///
/// Takes detected words and reconstructs a 2D table by:
/// 1. Detecting column positions (grouping by x-coordinate within `column_threshold`)
/// 2. Detecting row positions (grouping by y-center within `row_threshold_ratio` * median height)
/// 3. Assigning words to cells based on closest row/column
/// 4. Combining words within the same cell
///
/// Returns a `Vec<Vec<String>>` where each inner `Vec` is a row of cell texts.
pub(crate) fn reconstruct_table(
    words: &[HocrWord],
    column_threshold: u32,
    row_threshold_ratio: f64,
) -> Vec<Vec<String>> {
    if words.is_empty() {
        return Vec::new();
    }

    let col_positions = detect_columns(words, column_threshold);
    let row_positions = detect_rows(words, row_threshold_ratio);

    if col_positions.is_empty() || row_positions.is_empty() {
        return Vec::new();
    }

    let num_rows = row_positions.len();
    let num_cols = col_positions.len();
    let mut table: Vec<Vec<Vec<String>>> = vec![vec![vec![]; num_cols]; num_rows];

    for word in words {
        if let (Some(r), Some(c)) = (
            find_row_index(&row_positions, word),
            find_column_index(&col_positions, word),
        ) && r < num_rows
            && c < num_cols
        {
            table[r][c].push(word.text.clone());
        }
    }

    let result: Vec<Vec<String>> = table
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell_words| {
                    if cell_words.is_empty() {
                        String::new()
                    } else {
                        cell_words.join(" ")
                    }
                })
                .collect()
        })
        .collect();

    remove_empty_rows_and_columns(result)
}

/// Convert a table grid to markdown format.
///
/// The first row is treated as the header row, with a separator line added after it.
///
/// Delegates to the crate's single table renderer
/// ([`crate::rendering::common::render_table_markdown`]) so a reconstructed
/// OCR/PDF table serialises identically to one from any other source
/// (xberg-io/xberg#220). That renderer also pads every row to the widest row,
/// which this reconstruction path did not do — ragged rows used to emit fewer
/// pipe columns than the header, misaligning the table for downstream parsers.
pub(crate) fn table_to_markdown(table: &[Vec<String>]) -> String {
    crate::rendering::common::render_table_markdown(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_rows_zero_height_words_grouped_into_one_row() {
        let words = vec![
            HocrWord {
                text: "A".to_string(),
                left: 0,
                top: 10,
                width: 5,
                height: 0,
                confidence: 0.0,
            },
            HocrWord {
                text: "B".to_string(),
                left: 0,
                top: 10,
                width: 5,
                height: 0,
                confidence: 0.0,
            },
        ];
        let rows = detect_rows(&words, 0.5);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn test_nan_safe_sort_does_not_panic() {
        let mut values: Vec<f64> = vec![1.0, f64::NAN, 2.0];
        values.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(values.len(), 3);
        assert!(!values[0].is_nan());
        assert!(!values[1].is_nan());
        assert!(values[2].is_nan(), "NaN sorts last in ascending total_cmp order");
    }

    #[test]
    fn test_hocr_word_methods() {
        let word = HocrWord {
            text: "Hello".to_string(),
            left: 100,
            top: 50,
            width: 80,
            height: 30,
            confidence: 95.5,
        };

        assert_eq!(word.right(), 180);
        assert_eq!(word.bottom(), 80);
        assert_eq!(word.y_center(), 65.0);
        assert_eq!(word.x_center(), 140.0);
    }

    #[test]
    fn test_detect_columns() {
        let words = vec![
            HocrWord {
                text: "A".to_string(),
                left: 100,
                top: 50,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
            HocrWord {
                text: "B".to_string(),
                left: 300,
                top: 50,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
            HocrWord {
                text: "C".to_string(),
                left: 105,
                top: 100,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
            HocrWord {
                text: "D".to_string(),
                left: 295,
                top: 100,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
        ];

        let cols = detect_columns(&words, 20);
        assert_eq!(cols.len(), 2);
    }

    #[test]
    fn test_detect_rows() {
        let words = vec![
            HocrWord {
                text: "A".to_string(),
                left: 100,
                top: 50,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
            HocrWord {
                text: "B".to_string(),
                left: 200,
                top: 52,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
            HocrWord {
                text: "C".to_string(),
                left: 100,
                top: 100,
                width: 20,
                height: 30,
                confidence: 95.0,
            },
        ];

        let rows = detect_rows(&words, 0.5);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_reconstruct_table_basic() {
        let words = vec![
            HocrWord {
                text: "Name".to_string(),
                left: 100,
                top: 50,
                width: 40,
                height: 20,
                confidence: 95.0,
            },
            HocrWord {
                text: "Value".to_string(),
                left: 300,
                top: 50,
                width: 40,
                height: 20,
                confidence: 95.0,
            },
            HocrWord {
                text: "Alice".to_string(),
                left: 100,
                top: 100,
                width: 40,
                height: 20,
                confidence: 95.0,
            },
            HocrWord {
                text: "42".to_string(),
                left: 300,
                top: 100,
                width: 20,
                height: 20,
                confidence: 95.0,
            },
        ];

        let table = reconstruct_table(&words, 20, 0.5);
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].len(), 2);
        assert_eq!(table[0][0], "Name");
        assert_eq!(table[0][1], "Value");
        assert_eq!(table[1][0], "Alice");
        assert_eq!(table[1][1], "42");
    }

    #[test]
    fn test_table_to_markdown_basic() {
        let table = vec![
            vec!["Name".to_string(), "Value".to_string()],
            vec!["Alice".to_string(), "42".to_string()],
        ];

        let md = table_to_markdown(&table);
        assert!(md.contains("| Name | Value |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| Alice | 42 |"));
    }

    #[test]
    fn test_table_to_markdown_empty() {
        assert_eq!(table_to_markdown(&[]), String::new());
    }

    #[test]
    fn test_table_to_markdown_escapes_pipes() {
        let table = vec![vec!["Header".to_string()], vec!["a|b".to_string()]];

        let md = table_to_markdown(&table);
        assert!(md.contains("a\\|b"));
    }

    /// Regression test for issue where intra-cell word spacing ("Chose 1")
    /// was incorrectly split into separate columns.
    /// The word "1" should stay in the same cell as "Chose" despite having
    /// a different left position, because they're separated by a small gap.
    #[test]
    fn test_reconstruct_table_intra_cell_word_spacing() {
        let words = vec![
            HocrWord {
                text: "Chose".to_string(),
                left: 57,
                top: 496,
                width: 30,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "Truc".to_string(),
                left: 306,
                top: 496,
                width: 23,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "Chose".to_string(),
                left: 57,
                top: 510,
                width: 28,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "1".to_string(),
                left: 90,
                top: 510,
                width: 6,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "Truc".to_string(),
                left: 306,
                top: 510,
                width: 21,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "1".to_string(),
                left: 332,
                top: 510,
                width: 5,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "Chose".to_string(),
                left: 57,
                top: 524,
                width: 28,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "2".to_string(),
                left: 90,
                top: 524,
                width: 6,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "Truc".to_string(),
                left: 306,
                top: 524,
                width: 21,
                height: 12,
                confidence: 95.0,
            },
            HocrWord {
                text: "2".to_string(),
                left: 332,
                top: 524,
                width: 5,
                height: 12,
                confidence: 95.0,
            },
        ];

        let table = reconstruct_table(&words, 60, 0.5);

        assert_eq!(table.len(), 3, "Expected 3 rows, got {}", table.len());
        assert_eq!(table[0].len(), 2, "Expected 2 columns in row 0, got {}", table[0].len());

        assert_eq!(table[0][0], "Chose", "Header row 1, col 1");
        assert_eq!(table[0][1], "Truc", "Header row 1, col 2");
        assert_eq!(table[1][0], "Chose 1", "Row 2, col 1 should contain merged text");
        assert_eq!(table[1][1], "Truc 1", "Row 2, col 2 should contain merged text");
        assert_eq!(table[2][0], "Chose 2", "Row 3, col 1 should contain merged text");
        assert_eq!(table[2][1], "Truc 2", "Row 3, col 2 should contain merged text");
    }

    /// Regression/verification test for the claim in xberg-io/xberg#183 that
    /// `reconstruct_table` "drops any word for which `find_row_index` or
    /// `find_column_index` returns `None`".
    ///
    /// That claim does not hold for the current implementation: both
    /// `find_row_index` and `find_column_index` resolve to the *nearest*
    /// row/column via `min_by_key`, which is `Some` for every word whenever
    /// `row_positions`/`col_positions` are non-empty — and `reconstruct_table`
    /// already early-returns before this loop if either is empty. There is no
    /// path through this function that silently discards a word; even a word
    /// wildly outside the detected column/row bands is force-assigned to its
    /// nearest band instead of being dropped. This test proves that with an
    /// outlier word far outside the main table extent: every input word is
    /// still present, exactly once, in the reconstructed table.
    #[test]
    fn test_reconstruct_table_never_drops_words_including_far_outliers() {
        let mut words = vec![
            HocrWord {
                text: "A1".to_string(),
                left: 0,
                top: 0,
                width: 10,
                height: 10,
                confidence: 95.0,
            },
            HocrWord {
                text: "B1".to_string(),
                left: 200,
                top: 0,
                width: 10,
                height: 10,
                confidence: 95.0,
            },
            HocrWord {
                text: "A2".to_string(),
                left: 0,
                top: 200,
                width: 10,
                height: 10,
                confidence: 95.0,
            },
            HocrWord {
                text: "B2".to_string(),
                left: 200,
                top: 200,
                width: 10,
                height: 10,
                confidence: 95.0,
            },
        ];
        // Far outside every detected row/column band and every other word's
        // neighborhood — the scenario #183 claims gets silently dropped.
        words.push(HocrWord {
            text: "Outlier".to_string(),
            left: 50_000,
            top: 50_000,
            width: 10,
            height: 10,
            confidence: 95.0,
        });

        let input_word_count = words.len();
        let table = reconstruct_table(&words, 20, 0.5);

        let output_word_count: usize = table
            .iter()
            .flat_map(|row| row.iter())
            .flat_map(|cell| cell.split_whitespace())
            .count();

        assert_eq!(
            output_word_count, input_word_count,
            "every input word (including the far outlier) must appear exactly once in the output; \
             reconstruct_table's nearest-row/nearest-column assignment never returns None here"
        );

        let all_text: Vec<&str> = table
            .iter()
            .flat_map(|row| row.iter())
            .flat_map(|c| c.split_whitespace())
            .collect();
        assert!(
            all_text.contains(&"Outlier"),
            "the outlier word must not be silently dropped"
        );
    }
}
