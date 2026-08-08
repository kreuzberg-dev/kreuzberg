#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression test for #164: a genuinely blank CSV row mid-file must be kept
//! as its own row instead of being silently dropped, which used to shift
//! every subsequent row's index.

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// A blank line between two data rows must survive parsing as its own row.
#[tokio::test]
async fn should_keep_blank_row_mid_file() {
    let config = ExtractionConfig::default();

    let csv_content = b"Name,Age\nAlice,30\n\nBob,25";

    let extraction = extract_bytes_document(csv_content, "text/csv", &config)
        .await
        .expect("csv extraction should succeed");

    assert_eq!(extraction.tables.len(), 1, "CSV should have one table");
    let rows = &extraction.tables[0].cells;

    assert_eq!(rows.len(), 4, "expected header + blank + 2 data rows, got: {rows:?}");
    assert_eq!(rows[0], vec!["Name".to_string(), "Age".to_string()]);
    assert_eq!(rows[1], vec!["Alice".to_string(), "30".to_string()]);
    assert_eq!(
        rows[2],
        vec!["".to_string()],
        "blank row must be preserved, not dropped"
    );
    assert_eq!(rows[3], vec!["Bob".to_string(), "25".to_string()]);
}
