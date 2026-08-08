#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression test for #150: a stray, non-leading quote inside an unquoted CSV
//! field must not flip the parser into quote mode and swallow subsequent
//! delimiters/newlines.

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// A stray quote mid-field (`5" pipe`) must not swallow the rest of the file into
/// one cell. Each row must remain a separate row with the correct column count.
#[tokio::test]
async fn should_not_swallow_rows_on_stray_midfield_quote() {
    let config = ExtractionConfig::default();

    let csv_content = b"Size,Description\n5\" pipe,Steel\nOther,Plastic";

    let extraction = extract_bytes_document(csv_content, "text/csv", &config)
        .await
        .expect("csv extraction should succeed");

    assert_eq!(extraction.tables.len(), 1, "CSV should have one table");
    let rows = &extraction.tables[0].cells;

    assert_eq!(rows.len(), 3, "expected header + 2 data rows, got: {rows:?}");
    assert_eq!(rows[0], vec!["Size".to_string(), "Description".to_string()]);
    assert_eq!(rows[1], vec!["5\" pipe".to_string(), "Steel".to_string()]);
    assert_eq!(rows[2], vec!["Other".to_string(), "Plastic".to_string()]);
}
