#![cfg(feature = "office")]
//! Regression tests for #124: RST `.. figure::`, `.. list-table::`, and `.. csv-table::`
//! directive handling.
//!
//! Before this fix, `extract_text_from_rst`'s directive dispatch had no branches for these
//! three directives, so they fell through to the generic `.. ` catch-all which silently
//! discarded the directive line *and* its entire indented body (image URI, caption, table
//! rows). These tests assert the directives are now turned into proper structured content:
//! a figure becomes an image reference plus its caption text, and list-table/csv-table become
//! real table structures with the expected rows and cells.

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// `.. figure::` is an image directive with `:alt:`/`:width:` options followed by a caption
/// paragraph. The caption must be preserved as text, not dropped with the rest of the
/// directive body.
#[tokio::test]
async fn should_extract_figure_directive_with_caption() {
    let rst = r#"Intro paragraph.

.. figure:: chart.png
   :alt: A bar chart
   :width: 400px

   This is the figure caption text.

Outro paragraph.
"#
    .as_bytes();

    let result = extract_bytes_document(rst, "text/x-rst", &ExtractionConfig::default())
        .await
        .expect("figure directive should extract successfully");

    assert!(
        result.content.contains("This is the figure caption text."),
        "expected caption text in content, got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("Intro paragraph."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("Outro paragraph."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );

    // The figure's image reference (alt-text placeholder, mirroring plain `.. image::`
    // handling) must also be preserved rather than being dropped along with the caption.
    assert!(
        result.content.contains("[image: A bar chart]"),
        "expected figure image placeholder using :alt: text, got: {:?}",
        result.content
    );

    let uris = result.uris.as_deref().unwrap_or(&[]);
    assert!(
        uris.iter().any(|u| u.url.contains("chart.png")),
        "expected the figure image URI chart.png to be recorded, uris: {:?}",
        uris
    );
}

/// `.. list-table::` authors a table as a bullet-list-of-bullet-lists. It must be parsed into
/// a real table with exact row/cell contents, not left as an unordered list or raw text.
#[tokio::test]
async fn should_parse_list_table_directive_into_table_structure() {
    let rst = r#"Before table.

.. list-table::
   :header-rows: 1

   * - Name
     - Age
   * - Alice
     - 30
   * - Bob
     - 25

After table.
"#
    .as_bytes();

    let result = extract_bytes_document(rst, "text/x-rst", &ExtractionConfig::default())
        .await
        .expect("list-table directive should extract successfully");

    assert_eq!(
        result.tables.len(),
        1,
        "expected exactly one table, got: {:?}",
        result.tables
    );
    let table = &result.tables[0];
    assert_eq!(
        table.cells,
        vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
            vec!["Bob".to_string(), "25".to_string()],
        ],
        "expected exact list-table rows/cells, got: {:?}",
        table.cells
    );

    assert!(
        result.content.contains("Before table."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("After table."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );
}

/// `.. csv-table::` bodies are CSV-formatted data, optionally with a `:header:` option row,
/// and must be parsed into a real table structure (including quoted fields with embedded
/// commas).
#[tokio::test]
async fn should_parse_csv_table_directive_into_table_structure() {
    let rst = r#"Before table.

.. csv-table::
   :header: "Name", "City"

   "Alice", "Springfield, USA"
   "Bob", "Metropolis"

After table.
"#
    .as_bytes();

    let result = extract_bytes_document(rst, "text/x-rst", &ExtractionConfig::default())
        .await
        .expect("csv-table directive should extract successfully");

    assert_eq!(
        result.tables.len(),
        1,
        "expected exactly one table, got: {:?}",
        result.tables
    );
    let table = &result.tables[0];
    assert_eq!(
        table.cells,
        vec![
            vec!["Name".to_string(), "City".to_string()],
            vec!["Alice".to_string(), "Springfield, USA".to_string()],
            vec!["Bob".to_string(), "Metropolis".to_string()],
        ],
        "expected exact csv-table rows/cells (with the embedded comma preserved inside the \
         quoted field), got: {:?}",
        table.cells
    );

    assert!(
        result.content.contains("Before table."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );
    assert!(
        result.content.contains("After table."),
        "expected surrounding content preserved, got: {:?}",
        result.content
    );
}

/// An unrecognized directive should degrade to emitting its body text instead of vanishing,
/// while an actual RST comment (`.. ` with no `name::` form) must still be dropped.
#[tokio::test]
async fn should_degrade_unknown_directive_to_body_text_but_drop_plain_comments() {
    let rst = r#"Before.

.. some-unknown-directive:: arg

   This unknown-directive body text should survive.

.. This is a plain RST comment that must be dropped.

After.
"#
    .as_bytes();

    let result = extract_bytes_document(rst, "text/x-rst", &ExtractionConfig::default())
        .await
        .expect("unknown directive should extract successfully");

    assert!(
        result
            .content
            .contains("This unknown-directive body text should survive."),
        "expected unknown directive body to degrade to text, got: {:?}",
        result.content
    );
    assert!(
        !result.content.contains("plain RST comment that must be dropped"),
        "expected plain RST comment to remain dropped, got: {:?}",
        result.content
    );
}
