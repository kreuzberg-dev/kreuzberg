//! Regression tests for xberg-io/xberg#163 — generated markdown tables must
//! escape both pipes and newlines in cell content.
//!
//! An unescaped `|` splits one cell into two columns; an unescaped newline ends
//! the table row outright, so the tail of the cell becomes a new (malformed)
//! row. Only one of the crate's table renderers escaped pipes and none escaped
//! newlines. The single shared renderer now escapes `|` as `\|` and turns any
//! line break into `<br>`.
//!
//! Run with:
//!   cargo test -p xberg --test issue_163_table_cell_escaping
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
fn should_escape_pipe_in_cell_content() {
    let cells = grid(&[&["Header"], &["a|b"]]);

    let markdown = render(&cells);

    assert_eq!(
        markdown, "| Header |\n| --- |\n| a\\|b |\n",
        "a pipe inside a cell must be backslash-escaped, not left to split the column"
    );
}

#[test]
fn should_escape_pipe_in_header_content() {
    let cells = grid(&[&["left|right"], &["value"]]);

    let markdown = render(&cells);

    assert_eq!(markdown, "| left\\|right |\n| --- |\n| value |\n");
}

#[test]
fn should_replace_newline_in_cell_with_line_break_tag() {
    let cells = grid(&[&["Header"], &["line1\nline2"]]);

    let markdown = render(&cells);

    assert_eq!(
        markdown, "| Header |\n| --- |\n| line1<br>line2 |\n",
        "a newline inside a cell must not terminate the table row"
    );
}

#[test]
fn should_collapse_crlf_in_cell_to_a_single_line_break_tag() {
    let cells = grid(&[&["Header"], &["line1\r\nline2"]]);

    let markdown = render(&cells);

    assert_eq!(
        markdown, "| Header |\n| --- |\n| line1<br>line2 |\n",
        "CRLF must yield one <br>, not two"
    );
}

#[test]
fn should_replace_lone_carriage_return_in_cell_with_line_break_tag() {
    let cells = grid(&[&["Header"], &["line1\rline2"]]);

    let markdown = render(&cells);

    assert_eq!(markdown, "| Header |\n| --- |\n| line1<br>line2 |\n");
}

#[test]
fn should_escape_pipes_and_newlines_together() {
    let cells = grid(&[&["H1", "H2"], &["a|b\nc", "plain"]]);

    let markdown = render(&cells);

    assert_eq!(markdown, "| H1 | H2 |\n| --- | --- |\n| a\\|b<br>c | plain |\n");
}

#[test]
fn should_keep_every_rendered_line_a_well_formed_table_row() {
    let cells = grid(&[&["H1", "H2"], &["multi\nline\ncell", "x|y"]]);

    let markdown = render(&cells);

    // Three lines only — the embedded newlines must not have created new rows.
    let lines: Vec<&str> = markdown.lines().collect();
    assert_eq!(lines.len(), 3, "embedded newlines must not add rows: {markdown}");
    for line in &lines {
        assert!(line.starts_with('|'), "line must start with a pipe: {line}");
        assert!(line.ends_with('|'), "line must end with a pipe: {line}");
    }
}

#[test]
fn should_leave_cells_without_special_characters_untouched() {
    let cells = grid(&[&["Name", "Age"], &["Alice", "30"]]);

    let markdown = render(&cells);

    assert_eq!(markdown, "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n");
}
