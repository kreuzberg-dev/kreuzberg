//! CSV and TSV extractor.
//!
//! Parses CSV/TSV files into structured table data and clean text output.
//! Handles RFC 4180 quoted fields with embedded commas and newlines.

use std::sync::LazyLock;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::text::utf8_validation;
use crate::types::Table;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::{CsvMetadata, FormatMetadata, Metadata};
use async_trait::async_trait;

static DATE_RE_ISO: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap());
static DATE_RE_US: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\d{1,2}/\d{1,2}/\d{2,4}").unwrap());
static DATE_RE_EU: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\d{1,2}\.\d{1,2}\.\d{2,4}").unwrap());
#[cfg_attr(alef, alef(skip))]
/// CSV/TSV extractor with proper field parsing.
///
/// Replaces raw text passthrough with structured CSV parsing,
/// producing space-separated text output and populated `tables` field.
pub struct CsvExtractor;

impl CsvExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for CsvExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for CsvExtractor {
    fn name(&self) -> &str {
        "csv-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "CSV/TSV text extraction with table structure"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for CsvExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "csv", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        let text = decode_csv_bytes(content);
        let delimiter = if mime_type == "text/tab-separated-values" {
            '\t'
        } else {
            detect_delimiter(&text)
        };

        let rows = parse_csv(&text, delimiter);

        for row in &rows {
            budget.step()?;
            budget.add_cells(row.len())?;
            for cell in row {
                budget.check_entity(cell)?;
                budget.account_text(cell.len())?;
            }
        }

        let row_count = rows.len();
        let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let has_header = detect_header(&rows);
        let column_types = infer_column_types(&rows, has_header);

        let markdown = build_markdown_table(&rows, has_header);
        let columns = has_header.then(|| rows.first().cloned()).flatten();

        let table = Table {
            cells: rows,
            markdown,
            page_number: 1,
            bounding_box: None,
            columns,
            ..Default::default()
        };

        let csv_metadata = CsvMetadata {
            row_count: row_count as u32,
            column_count: col_count as u32,
            delimiter: if delimiter != ',' {
                Some(delimiter.to_string())
            } else {
                None
            },
            has_header,
            column_types: if column_types.is_empty() {
                None
            } else {
                Some(column_types)
            },
        };

        let mut builder = InternalDocumentBuilder::new("csv");
        let content_text = if has_header {
            render_csv_embedding_text(&table.cells)
        } else {
            render_table_plain_csv(&table.cells)
        };
        let table_element = builder.push_table(table, None, None);
        builder.set_text(table_element, &content_text);

        let mut doc = builder.build();
        doc.mime_type = mime_type.to_string();

        doc.metadata = Metadata {
            format: Some(FormatMetadata::Csv(csv_metadata)),
            ..Default::default()
        };

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "csv",
            "extraction complete"
        );
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/csv", "text/tab-separated-values"]
    }

    fn priority(&self) -> i32 {
        60
    }
}

/// Auto-detect CSV delimiter using consistency-based approach.
/// Tests each candidate delimiter and picks the one producing the most
/// consistent column count across sample lines.
fn detect_delimiter(text: &str) -> char {
    const CANDIDATES: &[char] = &[',', '\t', '|', ';'];
    let mut best_delimiter = ',';
    let mut best_score = 0usize;

    for &candidate in CANDIDATES {
        let sample: String = text.lines().take(10).collect::<Vec<_>>().join("\n");
        let rows = parse_csv(&sample, candidate);
        if rows.len() < 2 {
            continue;
        }
        let col_counts: Vec<usize> = rows.iter().map(|r| r.len()).collect();
        let first_count = col_counts[0];
        if first_count <= 1 {
            continue;
        }
        let consistent_rows = col_counts.iter().filter(|&&c| c == first_count).count();
        let score = consistent_rows * first_count;
        if score > best_score {
            best_score = score;
            best_delimiter = candidate;
        }
    }
    best_delimiter
}

/// Parse CSV text into rows of fields, handling RFC 4180 quoted fields.
fn parse_csv(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    current_field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current_field.push(c);
            }
        } else {
            match c {
                '"' => {
                    in_quotes = true;
                }
                c if c == delimiter => {
                    current_row.push(current_field.clone());
                    current_field.clear();
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    current_row.push(current_field.clone());
                    current_field.clear();
                    if !current_row.iter().all(|f| f.is_empty()) {
                        rows.push(current_row);
                    }
                    current_row = Vec::new();
                }
                '\n' => {
                    current_row.push(current_field.clone());
                    current_field.clear();
                    if !current_row.iter().all(|f| f.is_empty()) {
                        rows.push(current_row);
                    }
                    current_row = Vec::new();
                }
                _ => {
                    current_field.push(c);
                }
            }
        }
    }

    if !current_field.is_empty() || !current_row.is_empty() {
        current_row.push(current_field);
        if !current_row.iter().all(|f| f.is_empty()) {
            rows.push(current_row);
        }
    }

    rows
}

/// Decode raw CSV bytes with encoding detection.
///
/// Tries UTF-8 first (zero-copy fast path). When the bytes are not valid UTF-8,
/// attempts to detect and decode using common encodings (Shift-JIS, cp932,
/// windows-1252, etc.) using encoding_rs.
///
/// When the `quality` feature is enabled, uses chardetng for more sophisticated
/// encoding detection. Without it, tries common encodings in order.
fn decode_csv_bytes(content: &[u8]) -> String {
    if let Ok(s) = utf8_validation::from_utf8(content) {
        return crate::utils::strip_bom(s).to_string();
    }

    #[cfg(feature = "quality")]
    {
        crate::utils::strip_bom(&crate::utils::safe_decode(content, None)).to_string()
    }

    #[cfg(not(feature = "quality"))]
    {
        decode_csv_bytes_fallback(content)
    }
}

/// Fallback encoding detection for CSV files without the `quality` feature.
///
/// Tries common CSV encodings (Shift-JIS, cp932, windows-1252, etc.) in order,
/// selecting the first one that decodes without errors.
#[cfg(not(feature = "quality"))]
fn decode_csv_bytes_fallback(content: &[u8]) -> String {
    let encoding_labels = [
        "shift_jis",
        "windows-31j",
        "gb18030",
        "big5",
        "windows-1252",
        "iso-8859-1",
    ];

    for label in &encoding_labels {
        if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (decoded, _, had_errors) = encoding.decode(content);
            if !had_errors {
                return decoded.into_owned();
            }
        }
    }

    if let Some(shift_jis) = encoding_rs::Encoding::for_label(b"shift_jis") {
        let (decoded, _, _) = shift_jis.decode(content);
        return decoded.into_owned();
    }

    String::from_utf8_lossy(content).into_owned()
}

/// Whether a CSV cell is a real number. `str::parse::<f64>` also accepts the
/// tokens "NaN", "inf", "infinity" (case-insensitive), so a header cell or a
/// column of those words would be misclassified as numeric — flipping header
/// and column-type detection (xberg-io/xberg#1223). Reject those spellings.
fn is_csv_number(cell: &str) -> bool {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let lower = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    if matches!(lower, "nan" | "inf" | "infinity") {
        return false;
    }
    trimmed.parse::<f64>().is_ok()
}

/// Detect whether the first row is a header row.
///
/// Heuristic: the first row is considered a header if:
/// - It has at least 2 columns
/// - No cell in the first row looks numeric (all text/labels)
/// - At least one cell in the data rows (rows 1-5) is numeric
fn detect_header(rows: &[Vec<String>]) -> bool {
    if rows.len() < 2 {
        return false;
    }

    let first_row = &rows[0];
    if first_row.len() < 2 {
        return false;
    }

    if first_row.iter().any(|cell| is_csv_number(cell)) {
        return false;
    }

    let data_rows = &rows[1..rows.len().min(6)];

    data_rows.iter().any(|row| row.iter().any(|cell| is_csv_number(cell)))
}

/// Infer column types by scanning the first N data rows.
///
/// Returns a vector of type strings: "numeric", "text", or "date" per column.
fn infer_column_types(rows: &[Vec<String>], has_header: bool) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return Vec::new();
    }

    let data_start = if has_header { 1 } else { 0 };
    let scan_end = rows.len().min(data_start + 20);
    if data_start >= scan_end {
        return vec!["text".to_string(); col_count];
    }

    let data_rows = &rows[data_start..scan_end];

    let date_patterns: &[&regex::Regex] = &[&DATE_RE_ISO, &DATE_RE_US, &DATE_RE_EU];

    (0..col_count)
        .map(|col_idx| {
            let mut numeric_count = 0usize;
            let mut date_count = 0usize;
            let mut non_empty_count = 0usize;

            for row in data_rows {
                let cell = row.get(col_idx).map(|s| s.trim()).unwrap_or("");
                if cell.is_empty() {
                    continue;
                }
                non_empty_count += 1;

                if is_csv_number(cell) {
                    numeric_count += 1;
                } else {
                    for re in date_patterns {
                        if re.is_match(cell) {
                            date_count += 1;
                            break;
                        }
                    }
                }
            }

            if non_empty_count == 0 {
                "text".to_string()
            } else if numeric_count * 2 >= non_empty_count {
                "numeric".to_string()
            } else if date_count * 2 >= non_empty_count {
                "date".to_string()
            } else {
                "text".to_string()
            }
        })
        .collect()
}

/// Render CSV rows as embedding-friendly header-value pairs.
///
/// Assumes the first row is a header. Each data row becomes a labeled block:
///
/// ```text
/// Row 1:
/// Name: Alice
/// Age: 30
///
/// Row 2:
/// Name: Bob
/// Age: 25
/// ```
///
/// Empty cells are skipped. Rows where all cells are empty are omitted entirely.
/// Rows shorter than the header silently skip the missing columns.
fn render_csv_embedding_text(cells: &[Vec<String>]) -> String {
    if cells.len() < 2 {
        if let Some(headers) = cells.first() {
            return headers
                .iter()
                .filter(|h| !h.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
        }
        return String::new();
    }

    let headers = &cells[0];
    let data_rows = &cells[1..];

    let mut out = String::new();
    let mut row_number = 0usize;

    for row in data_rows {
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }

        row_number += 1;

        if row_number > 1 {
            out.push_str("\n\n");
        }

        out.push_str("Row ");
        out.push_str(&row_number.to_string());
        out.push(':');

        for (col_idx, header) in headers.iter().enumerate() {
            let header = header.trim();
            if header.is_empty() {
                continue;
            }
            let value = row.get(col_idx).map(|v| v.trim()).unwrap_or("");
            if value.is_empty() {
                continue;
            }
            out.push('\n');
            out.push_str(header);
            out.push_str(": ");
            out.push_str(value);
        }
    }

    out
}

/// Render CSV rows as space-separated plain text (fallback when no header detected).
fn render_table_plain_csv(cells: &[Vec<String>]) -> String {
    cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a Markdown table from parsed rows.
fn build_markdown_table(rows: &[Vec<String>], has_header: bool) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return String::new();
    }

    let mut markdown = String::new();
    if !has_header {
        markdown.push('|');
        for _ in 0..col_count {
            markdown.push_str("  |");
        }
        markdown.push('\n');
        markdown.push('|');
        for _ in 0..col_count {
            markdown.push_str(" --- |");
        }
        markdown.push('\n');
    }

    for (i, row) in rows.iter().enumerate() {
        markdown.push('|');
        for j in 0..col_count {
            let cell = row.get(j).map(|s| s.trim()).unwrap_or("");
            markdown.push(' ');
            markdown.push_str(cell);
            markdown.push_str(" |");
        }
        markdown.push('\n');

        if has_header && i == 0 {
            markdown.push('|');
            for _ in 0..col_count {
                markdown.push_str(" --- |");
            }
            markdown.push('\n');
        }
    }

    markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csv_simple() {
        let rows = parse_csv("a,b,c\n1,2,3\n", ',');
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["1", "2", "3"]]);
    }

    #[test]
    fn test_parse_csv_quoted() {
        let rows = parse_csv("\"hello, world\",b,c\n", ',');
        assert_eq!(rows, vec![vec!["hello, world", "b", "c"]]);
    }

    #[test]
    fn test_parse_csv_escaped_quotes() {
        let rows = parse_csv("\"say \"\"hello\"\"\",b\n", ',');
        assert_eq!(rows, vec![vec!["say \"hello\"", "b"]]);
    }

    #[test]
    fn test_parse_tsv() {
        let rows = parse_csv("a\tb\tc\n1\t2\t3\n", '\t');
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["1", "2", "3"]]);
    }

    #[test]
    fn test_parse_csv_crlf() {
        let rows = parse_csv("a,b\r\n1,2\r\n", ',');
        assert_eq!(rows, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn test_parse_csv_empty_fields() {
        let rows = parse_csv("a,,c\n", ',');
        assert_eq!(rows, vec![vec!["a", "", "c"]]);
    }

    #[test]
    fn test_build_markdown_table() {
        let rows = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];
        let md = build_markdown_table(&rows, true);
        assert!(md.contains("| Name | Age |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| Alice | 30 |"));
    }

    #[test]
    fn should_build_markdown_with_empty_header_for_headerless_rows() {
        let rows = vec![
            vec!["Alice".to_string(), "NYC".to_string()],
            vec!["Bob".to_string(), "LA".to_string()],
        ];

        let markdown = build_markdown_table(&rows, false);

        assert!(markdown.starts_with("|  |  |\n| --- | --- |\n"));
        assert!(markdown.contains("| Alice | NYC |"));
        assert!(markdown.contains("| Bob | LA |"));
    }

    #[tokio::test]
    async fn test_csv_extractor_plugin_interface() {
        let extractor = CsvExtractor::new();
        assert_eq!(extractor.name(), "csv-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert_eq!(
            extractor.supported_mime_types(),
            &["text/csv", "text/tab-separated-values"]
        );
    }

    #[tokio::test]
    async fn test_csv_extractor_output() {
        let extractor = CsvExtractor::new();
        let config = ExtractionConfig::default();
        let csv_data = b"Name,Age,City\nAlice,30,NYC\nBob,25,LA\n";

        let result = extractor
            .extract_content(csv_data, "text/csv", &config)
            .await
            .expect("CSV extraction should succeed");

        assert!(!result.tables.is_empty());
        assert!(matches!(
            result.elements.as_slice(),
            [crate::types::internal::InternalElement {
                kind: crate::types::internal::ElementKind::Table { table_index: 0 },
                ..
            }]
        ));
        let markdown = crate::rendering::render_markdown(&result);
        assert!(markdown.contains("| Name | Age | City |"));
        assert!(markdown.contains("| Alice | 30 | NYC |"));
        assert!(!markdown.contains("Row 1:"));

        let plain = crate::rendering::render_plain(&result);
        assert!(plain.contains("Row 1:\nName: Alice\nAge: 30\nCity: NYC"));
        assert!(plain.contains("Row 2:\nName: Bob\nAge: 25\nCity: LA"));
        assert!(!plain.contains('|'));

        if let Some(FormatMetadata::Csv(csv_meta)) = &result.metadata.format {
            assert!(csv_meta.has_header);
        } else {
            panic!("Expected FormatMetadata::Csv");
        }
    }

    #[tokio::test]
    async fn should_render_headerless_csv_without_promoting_first_data_row() {
        let extractor = CsvExtractor::new();
        let config = ExtractionConfig::default();
        let csv_data = b"Alice,NYC,Engineer\nBob,LA,Designer\n";

        let result = extractor
            .extract_content(csv_data, "text/csv", &config)
            .await
            .expect("CSV extraction should succeed");

        let markdown = crate::rendering::render_markdown(&result);
        assert!(markdown.starts_with("|  |  |  |\n| --- | --- | --- |\n"));
        assert!(markdown.contains("| Alice | NYC | Engineer |"));
        assert!(markdown.contains("| Bob | LA | Designer |"));

        let plain = crate::rendering::render_plain(&result);
        assert_eq!(plain, "Alice NYC Engineer\nBob LA Designer");
    }

    #[tokio::test]
    async fn test_csv_extractor_quoted_fields() {
        let extractor = CsvExtractor::new();
        let config = ExtractionConfig::default();
        let csv_data = b"Name,Description\n\"Smith, John\",\"Has a comma, inside\"\n";

        let result = extractor
            .extract_content(csv_data, "text/csv", &config)
            .await
            .expect("CSV extraction with quoted fields should succeed");

        assert!(!result.tables.is_empty());
    }

    #[test]
    fn test_detect_delimiter_comma() {
        assert_eq!(detect_delimiter("a,b,c\n1,2,3\n4,5,6"), ',');
    }

    #[test]
    fn test_detect_delimiter_semicolon() {
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n4;5;6"), ';');
    }

    #[test]
    fn test_detect_delimiter_pipe() {
        assert_eq!(detect_delimiter("a|b|c\n1|2|3\n4|5|6"), '|');
    }

    #[test]
    fn test_detect_delimiter_tab() {
        assert_eq!(detect_delimiter("a\tb\tc\n1\t2\t3\n4\t5\t6"), '\t');
    }

    #[test]
    fn test_detect_delimiter_semicolons_with_commas_in_values() {
        assert_eq!(
            detect_delimiter("\"last, first\";age;city\n\"doe, john\";30;NYC\n\"smith, jane\";25;LA"),
            ';'
        );
    }

    #[test]
    fn test_decode_csv_bytes_shift_jis() {
        let shift_jis_data = vec![
            0x96u8, 0xbc, 0x91, 0x4f, 0x2c, 0x94, 0x4e, 0x97, 0xee, 0x2c, 0x8f, 0x5a, 0x8f, 0x8a,
        ];

        let decoded = decode_csv_bytes(&shift_jis_data);

        assert!(decoded.contains("名前"), "Should contain '名前' (Name)");
        assert!(decoded.contains("年齢"), "Should contain '年齢' (Age)");
        assert!(decoded.contains("住所"), "Should contain '住所' (Address)");

        assert!(
            !decoded.contains("□"),
            "Should not contain mojibake replacement characters"
        );
        assert!(
            !decoded.contains("\u{FFFD}"),
            "Should not contain Unicode replacement characters"
        );
    }

    #[test]
    fn test_decode_csv_bytes_utf8() {
        let utf8_data = "名前,年齢,住所".as_bytes();
        let decoded = decode_csv_bytes(utf8_data);
        assert_eq!(decoded, "名前,年齢,住所");
    }

    #[test]
    fn test_detect_header_with_numeric_data() {
        let rows = vec![
            vec!["Name".to_string(), "Age".to_string(), "Score".to_string()],
            vec!["Alice".to_string(), "30".to_string(), "95.5".to_string()],
            vec!["Bob".to_string(), "25".to_string(), "88.0".to_string()],
        ];
        assert!(detect_header(&rows), "Should detect header when data rows have numbers");
    }

    #[test]
    fn test_detect_header_all_text() {
        let rows = vec![
            vec!["Name".to_string(), "City".to_string()],
            vec!["Alice".to_string(), "NYC".to_string()],
            vec!["Bob".to_string(), "LA".to_string()],
        ];
        assert!(!detect_header(&rows), "Should not detect header when all data is text");
    }

    #[test]
    fn test_detect_header_numeric_first_row() {
        let rows = vec![
            vec!["1".to_string(), "2".to_string(), "3".to_string()],
            vec!["4".to_string(), "5".to_string(), "6".to_string()],
        ];
        assert!(
            !detect_header(&rows),
            "Should not detect header when first row has numbers"
        );
    }

    #[test]
    fn nan_inf_are_not_numeric() {
        assert!(!is_csv_number("NaN"));
        assert!(!is_csv_number("inf"));
        assert!(!is_csv_number("-Infinity"));
        assert!(!is_csv_number("nan"));
        assert!(is_csv_number("42"));
        assert!(is_csv_number("-3.14"));
        assert!(is_csv_number("1e6"));
    }

    #[test]
    fn header_row_of_nan_inf_labels_still_detected_as_header() {
        let rows = vec![
            vec!["NaN".to_string(), "inf".to_string(), "label".to_string()],
            vec!["1".to_string(), "2".to_string(), "x".to_string()],
        ];
        assert!(
            detect_header(&rows),
            "header of NaN/inf/label words must be treated as a header, not numeric data"
        );
    }

    #[test]
    fn test_infer_column_types_basic() {
        let rows = vec![
            vec!["Name".to_string(), "Age".to_string(), "Date".to_string()],
            vec!["Alice".to_string(), "30".to_string(), "2024-01-15".to_string()],
            vec!["Bob".to_string(), "25".to_string(), "2024-02-20".to_string()],
        ];
        let types = infer_column_types(&rows, true);
        assert_eq!(types.len(), 3);
        assert_eq!(types[0], "text");
        assert_eq!(types[1], "numeric");
        assert_eq!(types[2], "date");
    }

    #[tokio::test]
    async fn test_csv_extractor_header_detection_metadata() {
        let extractor = CsvExtractor::new();
        let config = ExtractionConfig::default();
        let csv_data = b"Name,Age,City\nAlice,30,NYC\nBob,25,LA\n";

        let result = extractor.extract_content(csv_data, "text/csv", &config).await.unwrap();

        if let Some(FormatMetadata::Csv(csv_meta)) = &result.metadata.format {
            assert!(csv_meta.has_header);
            assert!(csv_meta.column_types.is_some(), "Should have column_types metadata");
        } else {
            panic!("Expected FormatMetadata::Csv");
        }
    }

    #[tokio::test]
    async fn test_csv_extractor_real_file() {
        let test_file =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/csv/data_table.csv");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test CSV");
        let extractor = CsvExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor.extract_content(&content, "text/csv", &config).await.unwrap();

        assert!(!result.tables.is_empty());
    }
}
