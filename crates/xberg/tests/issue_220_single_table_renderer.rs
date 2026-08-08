//! Regression tests for xberg-io/xberg#220 — eight independent table-to-markdown
//! implementations meant the same table serialised differently depending on
//! which format it was extracted from, with different escaping and different
//! width handling in each copy.
//!
//! Seven of the eight now delegate to `rendering::common::render_table_markdown`.
//! These tests pin the canonical output shape and assert that a table reaching
//! the user through a *format extractor* is byte-identical to the same cells
//! rendered through the builder.
//!
//! Run with:
//!   cargo test -p xberg --test issue_220_single_table_renderer --features office
//! The `office` feature is required for the OrgMode cross-source parity test;
//! the builder-only tests compile and run without any features.

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
fn should_render_the_canonical_gfm_shape() {
    let cells = grid(&[&["Name", "Age"], &["Alice", "30"], &["Bob", "25"]]);

    assert_eq!(
        render(&cells),
        "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n",
        "one renderer, one output shape"
    );
}

#[test]
fn should_emit_a_separator_row_even_for_a_header_only_table() {
    // Several of the forked copies emitted the separator only when
    // `cells.len() > 1`, producing output that is not a GFM table at all.
    let cells = grid(&[&["A", "B"]]);

    assert_eq!(render(&cells), "| A | B |\n| --- | --- |\n");
}

#[test]
fn should_render_a_single_column_table() {
    let cells = grid(&[&["Header"], &["Data1"], &["Data2"]]);

    assert_eq!(render(&cells), "| Header |\n| --- |\n| Data1 |\n| Data2 |\n");
}

#[test]
fn should_preserve_unicode_and_markdown_punctuation_verbatim() {
    let cells = grid(&[&["Emoji", "*Header*"], &["🎉 Party", "**Bold**"]]);

    assert_eq!(
        render(&cells),
        "| Emoji | *Header* |\n| --- | --- |\n| 🎉 Party | **Bold** |\n",
        "only pipes and line breaks are escaped; other markdown punctuation is left alone"
    );
}

/// The OrgMode extractor used to own a private `cells_to_markdown` that neither
/// escaped pipes nor normalised widths. Its output must now be byte-identical
/// to the builder's for the same cells.
#[cfg(feature = "office")]
#[tokio::test]
async fn should_render_org_table_identically_to_the_builder() {
    use xberg::core::config::{ExtractInput, ExtractionConfig};
    use xberg::plugins::DocumentExtractor;

    let org = "| Name | Age |\n|------+-----|\n| Alice | 30 |\n";
    let input = ExtractInput::from_bytes(org.as_bytes().to_vec(), "text/x-org".to_string(), None);
    let config = ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    };

    let extracted = xberg::extractors::OrgModeExtractor
        .extract(input, &config)
        .await
        .expect("org extraction succeeds");

    assert_eq!(extracted.tables.len(), 1, "expected exactly one table");

    let expected = render(&grid(&[&["Name", "Age"], &["Alice", "30"]]));
    assert_eq!(
        extracted.tables[0].markdown, expected,
        "an Org table must serialise exactly like the same cells from any other source"
    );
    assert_eq!(
        extracted.tables[0].markdown,
        "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n"
    );
}

/// A pipe typed into an Org cell must be escaped on the way out — the OrgMode
/// copy of the renderer emitted it raw, silently adding a column.
#[cfg(feature = "office")]
#[tokio::test]
async fn should_escape_pipes_coming_from_an_org_table() {
    use xberg::core::config::{ExtractInput, ExtractionConfig};
    use xberg::plugins::DocumentExtractor;

    let org = "| Name | Note |\n|------+------|\n| Alice | a\\vert{}b |\n";
    let input = ExtractInput::from_bytes(org.as_bytes().to_vec(), "text/x-org".to_string(), None);
    let config = ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    };

    let extracted = xberg::extractors::OrgModeExtractor
        .extract(input, &config)
        .await
        .expect("org extraction succeeds");

    assert_eq!(extracted.tables.len(), 1, "expected exactly one table");
    let markdown = &extracted.tables[0].markdown;

    // Whatever the cell text ends up being, no data row may contain a bare pipe
    // beyond the three structural delimiters of a two-column row.
    for line in markdown.lines().skip(2) {
        let bare_pipes = line
            .char_indices()
            .filter(|(i, c)| *c == '|' && (*i == 0 || !line[..*i].ends_with('\\')))
            .count();
        assert_eq!(
            bare_pipes, 3,
            "a two-column row must have exactly 3 unescaped pipes: {line}"
        );
    }
}
