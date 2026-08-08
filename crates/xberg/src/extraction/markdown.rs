//! Markdown table formatting utilities
//!
//! This module provides utilities for converting tabular data into GitHub-Flavored Markdown (GFM) tables.
//! It's used by multiple extractors (DOCX, HTML) that need to represent structured table data in markdown format.

use crate::extraction::capacity;

/// Converts a 2D vector of cell strings into a GitHub-Flavored Markdown table.
///
/// # Behavior
///
/// - The first row is treated as the header row
/// - A separator row is inserted after the header
/// - Pipe characters (`|`) in cell content are automatically escaped with backslash
/// - Line breaks in cell content become `<br>` so a cell cannot end its own row
/// - Irregular tables (rows with varying column counts) are normalised to the widest row: short rows
///   are padded with empty cells and no cell of an over-wide row is discarded
/// - Returns an empty string for empty input
///
/// # Arguments
///
/// * `cells` - A slice of vectors representing table rows, where each inner vector contains cell values
///
/// # Returns
///
/// A `String` containing the GFM markdown table representation
///
/// # Examples
///
/// ```ignore
/// # use xberg::extraction::cells_to_markdown;
/// let cells = vec![
///     vec!["Name".to_string(), "Age".to_string()],
///     vec!["Alice".to_string(), "30".to_string()],
///     vec!["Bob".to_string(), "25".to_string()],
/// ];
///
/// let markdown = cells_to_markdown(&cells);
/// assert!(markdown.contains("| Name | Age |"));
/// assert!(markdown.contains("| --- | --- |"));
/// ```
///
/// Converts a 2D vector of cell strings into plain text with tab-separated columns.
///
/// # Behavior
///
/// - Rows are separated by newlines
/// - Cells within a row are separated by tab characters
/// - No pipe delimiters or separator rows (unlike markdown tables)
/// - Returns an empty string for empty input
///
/// # Arguments
///
/// * `cells` - A slice of vectors representing table rows, where each inner vector contains cell values
///
/// # Returns
///
/// A `String` containing the plain text table representation
pub(crate) fn cells_to_text(cells: &[Vec<String>]) -> String {
    if cells.is_empty() {
        return String::new();
    }

    let estimated_capacity = cells
        .iter()
        .map(|r| r.iter().map(|c| c.len() + 1).sum::<usize>())
        .sum::<usize>();
    let mut text = String::with_capacity(estimated_capacity);

    for row in cells {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                text.push('\t');
            }
            text.push_str(cell);
        }
        text.push('\n');
    }

    text
}

/// Delegates to the crate's single table renderer
/// ([`crate::rendering::common::render_table_markdown`]) so office/XML
/// extractors emit byte-identical tables to every other source
/// (xberg-io/xberg#220).
///
/// This wrapper previously sized the grid from the *header* row and dropped any
/// cell past that width, silently losing data from rows wider than their header
/// (xberg-io/xberg#221). The shared renderer sizes from the widest row.
pub(crate) fn cells_to_markdown(cells: &[Vec<String>]) -> String {
    let num_cols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let estimated_capacity = capacity::estimate_table_markdown_capacity(cells.len(), num_cols);
    let mut markdown = String::with_capacity(estimated_capacity);
    crate::rendering::common::render_table_markdown_into(&mut markdown, cells);
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_formatting_from_simple_table() {
        let cells = vec![
            vec!["Header1".to_string(), "Header2".to_string()],
            vec!["Row1Col1".to_string(), "Row1Col2".to_string()],
            vec!["Row2Col1".to_string(), "Row2Col2".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("| Header1 | Header2 |"));
        assert!(markdown.contains("| --- | --- |"));
        assert!(markdown.contains("| Row1Col1 | Row1Col2 |"));
        assert!(markdown.contains("| Row2Col1 | Row2Col2 |"));

        let lines: Vec<&str> = markdown.lines().collect();
        assert_eq!(lines.len(), 4);
    }

    /// xberg-io/xberg#221: this function sized the grid from the header row and
    /// `break`-ed out of any wider data row, discarding the overflow cells. It
    /// is the only call site where the loss was unrecoverable, so it gets an
    /// exact-value test of its own.
    #[test]
    fn should_keep_every_cell_when_row_is_wider_than_header() {
        let cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string(), "3".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert_eq!(markdown, "| A | B |  |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n");
        assert!(markdown.contains(" 3 "), "the overflow cell must survive: {markdown}");
    }

    /// xberg-io/xberg#163: cell content must not be able to end its own row.
    #[test]
    fn should_escape_pipes_and_line_breaks_in_cells() {
        let cells = vec![
            vec!["H1".to_string(), "H2".to_string()],
            vec!["a|b".to_string(), "x\ny".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert_eq!(markdown, "| H1 | H2 |\n| --- | --- |\n| a\\|b | x<br>y |\n");
    }

    #[test]
    fn test_markdown_handles_empty_input() {
        let cells: Vec<Vec<String>> = vec![];

        let markdown = cells_to_markdown(&cells);

        assert_eq!(markdown, "");
    }

    #[test]
    fn test_markdown_escapes_pipe_characters() {
        let cells = vec![vec!["Header".to_string()], vec!["Cell with | pipe".to_string()]];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("Cell with \\| pipe"));

        for line in markdown.lines() {
            if !line.is_empty() {
                assert!(line.starts_with('|'));
                assert!(line.ends_with('|'));
            }
        }
    }

    #[test]
    fn test_markdown_pads_irregular_tables() {
        let cells = vec![
            vec!["H1".to_string(), "H2".to_string(), "H3".to_string()],
            vec!["R1C1".to_string(), "R1C2".to_string()],
            vec!["R2C1".to_string(), "R2C2".to_string(), "R2C3".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("| H1 | H2 | H3 |"));

        assert!(markdown.contains("| R1C1 | R1C2 |  |"));

        let lines: Vec<&str> = markdown.lines().filter(|l| !l.is_empty()).collect();
        let pipe_counts: Vec<usize> = lines
            .iter()
            .map(|line| line.chars().filter(|c| *c == '|').count())
            .collect();
        assert!(pipe_counts.iter().all(|&count| count == pipe_counts[0]));
    }

    #[test]
    fn test_markdown_single_row_table() {
        let cells = vec![vec!["OnlyHeader".to_string()]];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("| OnlyHeader |"));
        assert!(markdown.contains("| --- |"));

        let lines: Vec<&str> = markdown.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_markdown_single_column_table() {
        let cells = vec![
            vec!["Header".to_string()],
            vec!["Data1".to_string()],
            vec!["Data2".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("| Header |"));
        assert!(markdown.contains("| --- |"));
        assert!(markdown.contains("| Data1 |"));
        assert!(markdown.contains("| Data2 |"));
    }

    #[test]
    fn test_markdown_special_characters() {
        let cells = vec![
            vec!["*Header*".to_string(), "#Title".to_string()],
            vec!["**Bold**".to_string(), "~~Strike~~".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("*Header*"));
        assert!(markdown.contains("#Title"));
        assert!(markdown.contains("**Bold**"));
        assert!(markdown.contains("~~Strike~~"));
    }

    #[test]
    fn test_markdown_unicode_content() {
        let cells = vec![
            vec!["Emoji".to_string(), "Accents".to_string()],
            vec!["🎉 Party".to_string(), "Café".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);

        assert!(markdown.contains("🎉 Party"));
        assert!(markdown.contains("Café"));
    }

    #[test]
    fn test_text_simple_table() {
        let cells = vec![
            vec!["Header1".to_string(), "Header2".to_string()],
            vec!["Row1Col1".to_string(), "Row1Col2".to_string()],
        ];

        let text = cells_to_text(&cells);

        assert_eq!(text, "Header1\tHeader2\nRow1Col1\tRow1Col2\n");
    }

    #[test]
    fn test_text_empty_input() {
        let cells: Vec<Vec<String>> = vec![];
        assert_eq!(cells_to_text(&cells), "");
    }

    #[test]
    fn test_text_single_column() {
        let cells = vec![vec!["A".to_string()], vec!["B".to_string()]];

        let text = cells_to_text(&cells);
        assert_eq!(text, "A\nB\n");
    }

    #[test]
    fn test_text_no_pipe_delimiters() {
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];

        let text = cells_to_text(&cells);
        assert!(!text.contains('|'));
        assert!(!text.contains("---"));
    }

    #[test]
    fn test_text_preserves_pipe_chars_in_content() {
        let cells = vec![vec!["A | B".to_string()]];

        let text = cells_to_text(&cells);
        assert!(text.contains("A | B"));
    }
}
