//! Regression tests for xberg-io/xberg#221 — a table row wider than its header
//! silently lost every cell past the header's width.
//!
//! The old `extraction::markdown::cells_to_markdown` sized the grid from
//! `cells[0].len()` and then did `if idx >= num_cols { break; }` while walking a
//! data row, so a 3-cell row under a 2-cell header rendered as 2 cells and the
//! third value never reached the user. Every table renderer in the crate now
//! routes through one implementation that sizes the grid from the *widest* row.
//!
//! Run with:
//!   cargo test -p xberg --test issue_221_table_row_wider_than_header
//! (no `--features` needed: the builder and the renderer are both ungated)

use xberg::types::internal_builder::InternalDocumentBuilder;

/// Render a cell grid through the public builder and return the table markdown.
fn render(cells: &[Vec<String>]) -> String {
    let mut builder = InternalDocumentBuilder::new("test");
    builder.push_table_from_cells(cells, None, None);
    let doc = builder.build();
    assert_eq!(doc.tables.len(), 1, "expected exactly one table");
    doc.tables[0].markdown.clone()
}

/// Build a grid from string literals without the `to_string()` noise.
fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|c| (*c).to_string()).collect())
        .collect()
}

#[test]
fn should_keep_every_cell_when_row_is_wider_than_header() {
    let cells = grid(&[&["A", "B"], &["1", "2", "3"]]);

    let markdown = render(&cells);

    // The grid widens to 3 columns: the header is padded, the separator gains a
    // third column, and "3" survives.
    assert_eq!(
        markdown, "| A | B |  |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n",
        "row wider than the header must keep every cell"
    );
    assert!(
        markdown.contains(" 3 "),
        "the overflow cell must not be dropped: {markdown}"
    );
}

#[test]
fn should_keep_every_cell_when_row_is_much_wider_than_header() {
    let cells = grid(&[&["only"], &["a", "b", "c", "d"]]);

    let markdown = render(&cells);

    assert_eq!(
        markdown, "| only |  |  |  |\n| --- | --- | --- | --- |\n| a | b | c | d |\n",
        "all four cells must survive a single-column header"
    );
}

#[test]
fn should_pad_rows_narrower_than_the_widest_row() {
    let cells = grid(&[&["H1", "H2", "H3"], &["R1C1", "R1C2"], &["R2C1", "R2C2", "R2C3"]]);

    let markdown = render(&cells);

    assert_eq!(
        markdown, "| H1 | H2 | H3 |\n| --- | --- | --- |\n| R1C1 | R1C2 |  |\n| R2C1 | R2C2 | R2C3 |\n",
        "short rows are padded so every line has the same column count"
    );
}

#[test]
fn should_give_every_rendered_line_the_same_column_count() {
    let cells = grid(&[&["A", "B"], &["1", "2", "3"], &["x"]]);

    let markdown = render(&cells);

    let pipe_counts: Vec<usize> = markdown
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.chars().filter(|c| *c == '|').count())
        .collect();
    assert_eq!(
        pipe_counts,
        vec![4, 4, 4, 4],
        "header, separator and every data row must be 3 columns wide: {markdown}"
    );
}

#[test]
fn should_render_empty_grid_as_empty_string() {
    let cells: Vec<Vec<String>> = Vec::new();

    assert_eq!(render(&cells), "");
}
