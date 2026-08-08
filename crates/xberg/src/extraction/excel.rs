//! Excel and spreadsheet extraction functions.
//!
//! This module provides Excel file parsing using the `calamine` library.
//! Supports both modern Office Open XML formats (.xlsx, .xlsm, .xlam, .xltm, .xlsb)
//! and legacy binary formats (.xls, .xla), as well as OpenDocument spreadsheets (.ods).
//!
//! # Features
//!
//! - **Multiple formats**: XLSX, XLSM, XLS, XLSB, ODS
//! - **Sheet extraction**: Reads all sheets from workbook
//! - **Markdown conversion**: Converts spreadsheet data to Markdown tables
//! - **Office metadata**: Extracts core properties, custom properties (when `office` feature enabled)
//! - **Error handling**: Distinguishes between format errors and true I/O errors
//!
//! # Example
//!
//! ```ignore
//! use xberg::extraction::excel::read_excel_file;
//!
//! # fn example() -> xberg::Result<()> {
//! let (workbook, _warnings) = read_excel_file("data.xlsx")?;
//!
//! println!("Sheet count: {}", workbook.sheets.len());
//! for sheet in &workbook.sheets {
//!     println!("Sheet: {}", sheet.name);
//! }
//! # Ok(())
//! # }
//! ```
use calamine::{Data, DataRef, Range, Reader, SheetVisible, open_workbook_auto};
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use crate::core::diagnostics::push_warning;
use crate::error::{Result, XbergError};
use crate::extraction::capacity;
use crate::types::revisions::{DocumentRevision, RevisionDelta, RevisionKind};
use crate::types::{ExcelSheet, ExcelWorkbook, ProcessingWarning};

/// Maximum number of cells in a Range's bounding box before we consider it pathological.
/// This threshold is set to prevent OOM when processing files with sparse data at extreme
/// positions (e.g., Excel Solver files that have cells at A1 and XFD1048575).
///
/// 100 million cells at ~64 bytes each = ~6.4 GB, which is a reasonable upper limit.
const MAX_BOUNDING_BOX_CELLS: u64 = 100_000_000;

/// Maximum number of formula/hyperlink/comment entries recorded per sheet in
/// `ExcelWorkbook::metadata` before the list is truncated. Keeps the metadata
/// map bounded for a pathological sheet with tens of thousands of formulas or
/// hyperlinks (xberg-io/xberg#89); this is a defensive cap, not a
/// user-configurable limit.
const MAX_METADATA_ENTRIES_PER_SHEET: usize = 200;

#[cfg(feature = "office")]
use crate::extraction::office_metadata::{
    app_properties::{DOC_SECURITY_KEY, decode_doc_security_flags},
    extract_core_properties, extract_custom_properties, extract_xlsx_app_properties,
};
#[cfg(feature = "office")]
use serde_json::Value;

/// Result of reading a spreadsheet: the parsed workbook plus any
/// [`ProcessingWarning`]s accumulated while reading it (a sheet that failed
/// to parse, a worksheet range that could not be read, or output truncated
/// against an internal safety cap). See the module doc on
/// `core::diagnostics` for the warning convention this follows.
pub(crate) type ExcelReadResult = (ExcelWorkbook, Vec<ProcessingWarning>);

pub(crate) fn read_excel_file(file_path: &str) -> Result<ExcelReadResult> {
    let lower_path = file_path.to_lowercase();
    let mut warnings: Vec<ProcessingWarning> = Vec::new();

    #[cfg(feature = "office")]
    let office_metadata = if lower_path.ends_with(".xlsx")
        || lower_path.ends_with(".xlsm")
        || lower_path.ends_with(".xlam")
        || lower_path.ends_with(".xltm")
    {
        extract_xlsx_office_metadata_from_file(file_path).ok()
    } else if lower_path.ends_with(".ods") {
        extract_ods_office_metadata_from_file(file_path).ok()
    } else {
        None
    };

    #[cfg(not(feature = "office"))]
    let office_metadata: Option<HashMap<String, String>> = None;

    if lower_path.ends_with(".xlsx") || lower_path.ends_with(".xlsm") || lower_path.ends_with(".xltm") {
        let file = std::fs::File::open(file_path)?;
        let workbook = calamine::Xlsx::new(std::io::BufReader::new(file))
            .map_err(|e| XbergError::parsing(format!("Failed to parse XLSX: {}", e)))?;
        let mut result = process_xlsx_workbook(workbook, office_metadata, &mut warnings)?;
        result.revisions = extract_xlsx_revisions_from_file(file_path);
        if let Some(comments) = extract_xlsx_comments_from_file(file_path) {
            result.metadata.insert("comments".to_owned(), comments);
        }
        return Ok((result, warnings));
    }

    if lower_path.ends_with(".xlam") {
        let file = std::fs::File::open(file_path)?;
        match calamine::Xlsx::new(std::io::BufReader::new(file)) {
            Ok(workbook) => {
                let result = process_xlsx_workbook(workbook, office_metadata, &mut warnings)?;
                return Ok((result, warnings));
            }
            Err(e) => {
                push_warning(
                    &mut warnings,
                    "excel",
                    format!("Workbook could not be parsed as XLSX and no sheets were extracted ({e})"),
                );
                return Ok((
                    ExcelWorkbook {
                        sheets: vec![],
                        metadata: office_metadata.unwrap_or_default(),
                        revisions: None,
                    },
                    warnings,
                ));
            }
        }
    }

    if lower_path.ends_with(".xla") {
        let file = std::fs::File::open(file_path)?;
        match calamine::Xls::new(std::io::BufReader::new(file)) {
            Ok(workbook) => {
                let result = process_workbook(workbook, office_metadata, &mut warnings)?;
                return Ok((result, warnings));
            }
            Err(e) => {
                push_warning(
                    &mut warnings,
                    "excel",
                    format!("Workbook could not be parsed as XLS and no sheets were extracted ({e})"),
                );
                return Ok((
                    ExcelWorkbook {
                        sheets: vec![],
                        metadata: office_metadata.unwrap_or_default(),
                        revisions: None,
                    },
                    warnings,
                ));
            }
        }
    }

    if lower_path.ends_with(".xlsb") {
        let file = std::fs::File::open(file_path)?;
        let workbook = calamine::Xlsb::new(std::io::BufReader::new(file))
            .map_err(|e| XbergError::parsing(format!("Failed to parse XLSB: {}", e)))?;
        let result = process_workbook(workbook, office_metadata, &mut warnings)?;
        return Ok((result, warnings));
    }

    let workbook = match open_workbook_auto(Path::new(file_path)) {
        Ok(wb) => wb,
        Err(calamine::Error::Io(io_err)) => {
            if io_err.kind() == std::io::ErrorKind::InvalidData {
                return Err(XbergError::parsing(format!(
                    "Cannot detect Excel file format: {}",
                    io_err
                )));
            }
            // Real IO error - bubble up unchanged ~keep
            return Err(io_err.into());
        }
        Err(e) => return Err(XbergError::parsing(format!("Failed to parse Excel file: {}", e))),
    };

    let result = process_workbook(workbook, office_metadata, &mut warnings)?;
    Ok((result, warnings))
}

pub(crate) fn read_excel_bytes(data: &[u8], file_extension: &str) -> Result<ExcelReadResult> {
    let mut warnings: Vec<ProcessingWarning> = Vec::new();

    #[cfg(feature = "office")]
    let office_metadata = match file_extension.to_lowercase().as_str() {
        ".xlsx" | ".xlsm" | ".xlam" | ".xltm" => extract_xlsx_office_metadata_from_bytes(data).ok(),
        ".ods" => extract_ods_office_metadata_from_bytes(data).ok(),
        _ => None,
    };

    #[cfg(not(feature = "office"))]
    let office_metadata: Option<HashMap<String, String>> = None;

    match file_extension.to_lowercase().as_str() {
        ".xlsx" | ".xlsm" | ".xltm" => {
            let cursor = Cursor::new(data);
            let workbook =
                calamine::Xlsx::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to parse XLSX: {}", e)))?;
            let mut result = process_xlsx_workbook(workbook, office_metadata, &mut warnings)?;
            result.revisions = extract_xlsx_revisions_from_bytes(data);
            if let Some(comments) = extract_xlsx_comments_from_bytes(data) {
                result.metadata.insert("comments".to_owned(), comments);
            }
            Ok((result, warnings))
        }
        ".xlam" => {
            let cursor = Cursor::new(data);
            match calamine::Xlsx::new(cursor) {
                Ok(workbook) => {
                    let result = process_xlsx_workbook(workbook, office_metadata, &mut warnings)?;
                    Ok((result, warnings))
                }
                Err(e) => {
                    push_warning(
                        &mut warnings,
                        "excel",
                        format!("Workbook could not be parsed as XLSX and no sheets were extracted ({e})"),
                    );
                    Ok((
                        ExcelWorkbook {
                            sheets: vec![],
                            metadata: office_metadata.unwrap_or_default(),
                            revisions: None,
                        },
                        warnings,
                    ))
                }
            }
        }
        ".xls" => {
            let cursor = Cursor::new(data);
            let workbook =
                calamine::Xls::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to parse XLS: {}", e)))?;
            let result = process_workbook(workbook, office_metadata, &mut warnings)?;
            Ok((result, warnings))
        }
        ".xla" => {
            let cursor = Cursor::new(data);
            match calamine::Xls::new(cursor) {
                Ok(workbook) => {
                    let result = process_workbook(workbook, office_metadata, &mut warnings)?;
                    Ok((result, warnings))
                }
                Err(e) => {
                    push_warning(
                        &mut warnings,
                        "excel",
                        format!("Workbook could not be parsed as XLS and no sheets were extracted ({e})"),
                    );
                    Ok((
                        ExcelWorkbook {
                            sheets: vec![],
                            metadata: office_metadata.unwrap_or_default(),
                            revisions: None,
                        },
                        warnings,
                    ))
                }
            }
        }
        ".xlsb" => {
            let cursor = Cursor::new(data);
            let workbook =
                calamine::Xlsb::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to parse XLSB: {}", e)))?;
            let result = process_workbook(workbook, office_metadata, &mut warnings)?;
            Ok((result, warnings))
        }
        ".ods" => {
            let cursor = Cursor::new(data);
            let workbook =
                calamine::Ods::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to parse ODS: {}", e)))?;
            let result = process_workbook(workbook, office_metadata, &mut warnings)?;
            Ok((result, warnings))
        }
        _ => Err(XbergError::parsing(format!(
            "Unsupported file extension: {}",
            file_extension
        ))),
    }
}

/// Process XLSX workbooks with special handling for pathological sparse files.
///
/// This function uses calamine's `worksheet_cells_reader()` API to detect sheets with
/// extreme bounding boxes BEFORE allocating memory for the full Range. This prevents
/// OOM when processing files like Excel Solver files that have cells at both A1 and
/// XFD1048575, creating a bounding box of ~17 billion cells.
fn process_xlsx_workbook<RS: Read + Seek>(
    mut workbook: calamine::Xlsx<RS>,
    office_metadata: Option<HashMap<String, String>>,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<ExcelWorkbook> {
    let sheet_names = workbook.sheet_names();
    let mut sheets = Vec::with_capacity(sheet_names.len());
    let mut extra_metadata: HashMap<String, String> = HashMap::new();

    for name in &sheet_names {
        match process_xlsx_sheet_safe(&mut workbook, name, warnings) {
            Ok(sheet) => sheets.push(sheet),
            Err(e) => {
                tracing::warn!("Failed to process sheet '{}': {}", name, e);
                push_warning(
                    warnings,
                    "excel",
                    format!("Sheet '{name}' could not be processed and was skipped ({e})"),
                );
            }
        }

        if let Some(formulas) = collect_sheet_formulas(&mut workbook, name) {
            extra_metadata.insert(format!("formulas_{name}"), formulas);
        }
        if let Some(hyperlinks) = collect_sheet_hyperlinks(&mut workbook, name) {
            extra_metadata.insert(format!("hyperlinks_{name}"), hyperlinks);
        }
    }

    let mut metadata = extract_metadata(&workbook, &sheet_names, office_metadata);
    metadata.extend(extra_metadata);
    Ok(ExcelWorkbook {
        sheets,
        metadata,
        revisions: None,
    })
}

/// Process a single XLSX sheet safely by pre-checking the bounding box.
///
/// This function streams cells to compute the actual bounding box without allocating
/// a full Range, then only creates the Range if the bounding box is within safe limits.
fn process_xlsx_sheet_safe<RS: Read + Seek>(
    workbook: &mut calamine::Xlsx<RS>,
    sheet_name: &str,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<ExcelSheet> {
    match classify_xlsx_sheet_shape(workbook, sheet_name)? {
        XlsxSheetShape::Empty => return Ok(empty_excel_sheet(sheet_name)),
        XlsxSheetShape::Dense => {}
        XlsxSheetShape::Sparse => {
            let (cells, row_min, row_max, col_min, col_max) = collect_xlsx_sheet_cells(workbook, sheet_name)?;
            if cells.is_empty() {
                return Ok(empty_excel_sheet(sheet_name));
            }
            return process_sparse_sheet_from_cells(sheet_name, cells, row_min, row_max, col_min, col_max, warnings);
        }
    }

    let range = workbook
        .worksheet_range(sheet_name)
        .map_err(|e| XbergError::parsing(format!("Failed to parse sheet '{}': {}", sheet_name, e)))?;

    Ok(process_sheet(sheet_name, &range, warnings))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XlsxSheetShape {
    Empty,
    Dense,
    Sparse,
}

fn classify_xlsx_sheet_shape<RS: Read + Seek>(
    workbook: &mut calamine::Xlsx<RS>,
    sheet_name: &str,
) -> Result<XlsxSheetShape> {
    let mut cell_reader = workbook
        .worksheet_cells_reader(sheet_name)
        .map_err(|e| XbergError::parsing(format!("Failed to read sheet '{}': {}", sheet_name, e)))?;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;

    while let Ok(Some(cell)) = cell_reader.next_cell() {
        let (row, col) = cell.get_position();
        let (row_min, row_max, col_min, col_max) = bounds.get_or_insert((row, row, col, col));
        *row_min = (*row_min).min(row);
        *row_max = (*row_max).max(row);
        *col_min = (*col_min).min(col);
        *col_max = (*col_max).max(col);

        let row_count = u64::from(*row_max - *row_min + 1);
        let col_count = u64::from(*col_max - *col_min + 1);
        if row_count.saturating_mul(col_count) > MAX_BOUNDING_BOX_CELLS {
            return Ok(XlsxSheetShape::Sparse);
        }
    }

    Ok(if bounds.is_some() {
        XlsxSheetShape::Dense
    } else {
        XlsxSheetShape::Empty
    })
}

type XlsxOwnedCells = (Vec<((u32, u32), Data)>, u32, u32, u32, u32);

fn collect_xlsx_sheet_cells<RS: Read + Seek>(
    workbook: &mut calamine::Xlsx<RS>,
    sheet_name: &str,
) -> Result<XlsxOwnedCells> {
    let mut cell_reader = workbook
        .worksheet_cells_reader(sheet_name)
        .map_err(|e| XbergError::parsing(format!("Failed to read sheet '{}': {}", sheet_name, e)))?;
    let mut cells = Vec::new();
    let mut row_min = u32::MAX;
    let mut row_max = 0;
    let mut col_min = u32::MAX;
    let mut col_max = 0;

    while let Ok(Some(cell)) = cell_reader.next_cell() {
        let (row, col) = cell.get_position();
        row_min = row_min.min(row);
        row_max = row_max.max(row);
        col_min = col_min.min(col);
        col_max = col_max.max(col);
        cells.push(((row, col), owned_xlsx_cell_value(cell.get_value())));
    }

    Ok((cells, row_min, row_max, col_min, col_max))
}

fn owned_xlsx_cell_value(value: &DataRef<'_>) -> Data {
    match value {
        DataRef::Empty => Data::Empty,
        DataRef::String(value) => Data::String(value.clone()),
        DataRef::SharedString(value) => Data::String(value.to_string()),
        DataRef::Float(value) => Data::Float(*value),
        DataRef::Int(value) => Data::Int(*value),
        DataRef::Bool(value) => Data::Bool(*value),
        DataRef::DateTime(value) => Data::DateTime(*value),
        DataRef::DateTimeIso(value) => Data::DateTimeIso(value.clone()),
        DataRef::DurationIso(value) => Data::DurationIso(value.clone()),
        DataRef::Error(value) => Data::Error(value.clone()),
    }
}

fn empty_excel_sheet(sheet_name: &str) -> ExcelSheet {
    ExcelSheet {
        name: sheet_name.to_owned(),
        markdown: format!("## {}\n\n*Empty sheet*", sheet_name),
        row_count: 0,
        col_count: 0,
        cell_count: 0,
        table_cells: None,
    }
}

/// Process a sparse sheet directly from collected cells without creating a full Range.
///
/// This is used when the bounding box would exceed MAX_BOUNDING_BOX_CELLS.
/// Instead of creating a dense Range, we generate a markdown pipe table from the sparse cells.
fn process_sparse_sheet_from_cells(
    sheet_name: &str,
    cells: Vec<((u32, u32), Data)>,
    row_min: u32,
    row_max: u32,
    col_min: u32,
    col_max: u32,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<ExcelSheet> {
    let cell_count = cells.len();
    let bb_rows = (row_max - row_min + 1) as usize;
    let bb_cols = (col_max - col_min + 1) as usize;

    let mut col_set = std::collections::BTreeSet::new();
    let mut row_set = std::collections::BTreeSet::new();
    let mut cell_map: HashMap<(u32, u32), &Data> = HashMap::with_capacity(cells.len());

    for ((row, col), data) in &cells {
        if !matches!(data, Data::Empty) {
            col_set.insert(*col);
            row_set.insert(*row);
            cell_map.insert((*row, *col), data);
        }
    }

    let cols: Vec<u32> = col_set.into_iter().collect();
    let rows: Vec<u32> = row_set.into_iter().collect();

    if cols.is_empty() || rows.is_empty() {
        let markdown = format!("## {}\n\n*Empty sheet*", sheet_name);
        return Ok(ExcelSheet {
            name: sheet_name.to_owned(),
            markdown,
            row_count: bb_rows,
            col_count: bb_cols,
            cell_count,
            table_cells: None,
        });
    }

    const MAX_OUTPUT_ROWS: usize = 1000;
    const MAX_OUTPUT_COLS: usize = 50;
    let display_rows = rows.len().min(MAX_OUTPUT_ROWS);
    let display_cols = cols.len().min(MAX_OUTPUT_COLS);

    let mut markdown = String::with_capacity(500 + cell_count * 20);
    let mut table_cells: Vec<Vec<String>> = Vec::with_capacity(display_rows + 1);

    write!(markdown, "## {}\n\n", sheet_name).expect("write to String cannot fail");

    let first_row = rows[0];
    let mut header_cells = Vec::with_capacity(display_cols);
    markdown.push_str("| ");
    for (i, &col) in cols.iter().take(display_cols).enumerate() {
        if i > 0 {
            markdown.push_str(" | ");
        }
        let cell_str = cell_map
            .get(&(first_row, col))
            .map(|d| format_cell_to_string(d))
            .unwrap_or_default();
        escape_markdown_into(&mut markdown, &cell_str);
        header_cells.push(cell_str);
    }
    markdown.push_str(" |\n");
    table_cells.push(header_cells);

    markdown.push_str("| ");
    for i in 0..display_cols {
        if i > 0 {
            markdown.push_str(" | ");
        }
        markdown.push_str("---");
    }
    markdown.push_str(" |\n");

    for &row in rows.iter().skip(1).take(display_rows - 1) {
        let mut row_cells_vec = Vec::with_capacity(display_cols);
        markdown.push_str("| ");
        for (i, &col) in cols.iter().take(display_cols).enumerate() {
            if i > 0 {
                markdown.push_str(" | ");
            }
            let cell_str = cell_map
                .get(&(row, col))
                .map(|d| format_cell_to_string(d))
                .unwrap_or_default();
            escape_markdown_into(&mut markdown, &cell_str);
            row_cells_vec.push(cell_str);
        }
        markdown.push_str(" |\n");
        table_cells.push(row_cells_vec);
    }

    if rows.len() > MAX_OUTPUT_ROWS || cols.len() > MAX_OUTPUT_COLS {
        write!(
            markdown,
            "\n*Truncated: showing {}x{} of {}x{} cells*\n",
            display_rows,
            display_cols,
            rows.len(),
            cols.len()
        )
        .expect("write to String cannot fail");

        push_warning(
            warnings,
            "excel",
            format!(
                "Sheet '{sheet_name}' output truncated to {display_rows}x{display_cols} of {}x{} \
                 non-empty rows/columns (internal sparse-sheet output cap)",
                rows.len(),
                cols.len()
            ),
        );
    }

    Ok(ExcelSheet {
        name: sheet_name.to_owned(),
        markdown,
        row_count: bb_rows,
        col_count: bb_cols,
        cell_count,
        table_cells: Some(table_cells),
    })
}

fn process_workbook<RS, R>(
    mut workbook: R,
    office_metadata: Option<HashMap<String, String>>,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<ExcelWorkbook>
where
    RS: std::io::Read + std::io::Seek,
    R: Reader<RS>,
    // Forwarded from `process_named_sheets`, which names the cause in its warning.
    R::Error: std::fmt::Display,
{
    let sheet_names = workbook.sheet_names();
    let (sheets, extra_metadata) = process_named_sheets(&mut workbook, &sheet_names, warnings);

    let mut metadata = extract_metadata(&workbook, &sheet_names, office_metadata);
    metadata.extend(extra_metadata);

    Ok(ExcelWorkbook {
        sheets,
        metadata,
        revisions: None,
    })
}

/// Read each named sheet's range and formulas from a generic calamine `Reader`
/// (xls/xlsb/ods — the XLSX path has its own bounding-box-aware
/// `process_xlsx_sheet_safe`). Split out from [`process_workbook`] so a sheet
/// name that the backend cannot resolve to a range can be exercised directly
/// in tests without needing a corrupt on-disk fixture (xberg-io/xberg#103):
/// pass a real, open workbook plus a sheet-name list containing a name the
/// backend doesn't recognize, and the failure is real `calamine::Reader`
/// behavior, not test-only scaffolding.
///
/// A worksheet range that fails to read is skipped with a warning naming the
/// sheet instead of silently dropped, mirroring the XLSX-side handling in
/// [`process_xlsx_workbook`].
fn process_named_sheets<RS, R>(
    workbook: &mut R,
    sheet_names: &[String],
    warnings: &mut Vec<ProcessingWarning>,
) -> (Vec<ExcelSheet>, HashMap<String, String>)
where
    RS: std::io::Read + std::io::Seek,
    R: Reader<RS>,
    // The warning names the failure cause, so the reader's error type has to be
    // renderable. Every concrete calamine reader (Xlsx, Xls, Ods) satisfies this.
    R::Error: std::fmt::Display,
{
    let mut sheets = Vec::with_capacity(sheet_names.len());
    let mut extra_metadata: HashMap<String, String> = HashMap::new();

    for name in sheet_names {
        match workbook.worksheet_range(name) {
            Ok(range) => sheets.push(process_sheet(name, &range, warnings)),
            Err(e) => {
                tracing::warn!("Failed to read worksheet range '{}': {}", name, e);
                push_warning(
                    warnings,
                    "excel",
                    format!("Sheet '{name}' worksheet range could not be read and was skipped ({e})"),
                );
            }
        }

        if let Some(formulas) = collect_sheet_formulas(workbook, name) {
            extra_metadata.insert(format!("formulas_{name}"), formulas);
        }
    }

    (sheets, extra_metadata)
}

#[inline]
fn process_sheet(name: &str, range: &Range<Data>, warnings: &mut Vec<ProcessingWarning>) -> ExcelSheet {
    let (rows, cols) = range.get_size();
    let cell_count = range.used_cells().count();

    let estimated_capacity = 50 + (cols * 20) + (cell_count * 12);

    if rows == 0 || cols == 0 {
        let markdown = format!("## {}\n\n*Empty sheet*", name);
        ExcelSheet {
            name: name.to_owned(),
            markdown,
            row_count: rows,
            col_count: cols,
            cell_count,
            table_cells: None,
        }
    } else {
        let (markdown, table_cells) = generate_markdown_and_cells(name, range, estimated_capacity, warnings);
        ExcelSheet {
            name: name.to_owned(),
            markdown,
            row_count: rows,
            col_count: cols,
            cell_count,
            table_cells: Some(table_cells),
        }
    }
}

/// Generate both markdown and extracted cells in a single pass.
///
/// This function produces both the markdown representation and the structured
/// cell data simultaneously, avoiding the expensive markdown re-parsing that
/// was previously done in `sheets_to_tables()`.
///
/// Returns (markdown, table_cells) where table_cells is a 2D vector of strings.
fn generate_markdown_and_cells(
    sheet_name: &str,
    range: &Range<Data>,
    capacity: usize,
    warnings: &mut Vec<ProcessingWarning>,
) -> (String, Vec<Vec<String>>) {
    const MAX_REASONABLE_ROWS: usize = 100_000;

    let (declared_rows, _declared_cols) = range.get_size();

    if declared_rows > MAX_REASONABLE_ROWS {
        let actual_cell_count = range.used_cells().count();

        if actual_cell_count < 10_000 {
            let result_capacity = 100 + sheet_name.len();
            let mut result = String::with_capacity(result_capacity);
            write!(
                result,
                "## {}\n\n*Sheet has extreme declared dimensions ({} rows) with minimal actual data ({} cells). Skipping to prevent OOM.*",
                sheet_name, declared_rows, actual_cell_count
            ).unwrap();
            push_warning(
                warnings,
                "excel",
                format!(
                    "Sheet '{sheet_name}' declared {declared_rows} rows but only {actual_cell_count} cells \
                     contain data; sheet content was skipped to avoid excessive memory use"
                ),
            );
            return (result, Vec::new());
        }
    }

    let rows: Vec<_> = range.rows().collect();
    if rows.is_empty() {
        let result_capacity = 50 + sheet_name.len();
        let mut result = String::with_capacity(result_capacity);
        write!(result, "## {}\n\n*No data*", sheet_name).unwrap();
        return (result, Vec::new());
    }

    let header = &rows[0];
    let header_len = header.len();
    let row_count = rows.len();

    let table_capacity = capacity::estimate_table_markdown_capacity(row_count, header_len);

    let mut exact_size = 16 + sheet_name.len();

    exact_size += 2 + (header_len * 2);
    exact_size += header_len * 10;

    exact_size += 5 + (header_len * 5);

    exact_size += (row_count - 1) * (5 + header_len * 15);

    let mut markdown = String::with_capacity(exact_size.max(table_capacity).max(capacity));
    let mut cells: Vec<Vec<String>> = Vec::with_capacity(row_count);

    write!(markdown, "## {}\n\n", sheet_name).unwrap();

    let mut header_cells = Vec::with_capacity(header_len);
    markdown.push_str("| ");
    for (i, cell) in header.iter().enumerate() {
        if i > 0 {
            markdown.push_str(" | ");
        }
        let cell_str = format_cell_to_string(cell);
        escape_markdown_into(&mut markdown, &cell_str);
        header_cells.push(cell_str);
    }
    markdown.push_str(" |\n");
    cells.push(header_cells);

    markdown.push_str("| ");
    for i in 0..header_len {
        if i > 0 {
            markdown.push_str(" | ");
        }
        markdown.push_str("---");
    }
    markdown.push_str(" |\n");

    for row in rows.iter().skip(1) {
        let mut row_cells = Vec::with_capacity(header_len);
        markdown.push_str("| ");
        for i in 0..header_len {
            if i > 0 {
                markdown.push_str(" | ");
            }
            let cell_str = if let Some(cell) = row.get(i) {
                let cell_str = format_cell_to_string(cell);
                escape_markdown_into(&mut markdown, &cell_str);
                cell_str
            } else {
                String::new()
            };
            row_cells.push(cell_str);
        }
        markdown.push_str(" |\n");
        cells.push(row_cells);
    }

    (markdown, cells)
}

/// Convert a Data cell to its string representation.
///
/// This helper function is shared between markdown generation and cell extraction
/// to ensure byte-identical output.
///
/// Float values that are whole numbers (e.g. 1.0, 42.0) are formatted without the
/// trailing decimal point (e.g. "1", "42") so that numeric ground-truth comparisons
/// produce correct F1 scores.  Rust's default `{}` formatter already does this for
/// `f64`, so we simply delegate to it for every float case.
#[inline]
fn format_cell_to_string(data: &Data) -> String {
    match data {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format!("{}", f),
        Data::Int(i) => format!("{}", i),
        Data::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Data::DateTime(dt) => {
            let (year, month, day, hour, min, sec, _milli) = dt.to_ymd_hms_milli();
            format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, hour, min, sec)
        }
        Data::Error(e) => format!("#ERR: {:?}", e),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => format!("DURATION: {}", s),
    }
}

/// Stand-in for a line break inside a spreadsheet cell (Alt+Enter). A raw
/// newline would end the markdown table row, splitting one cell's content
/// across two rows (xberg-io/xberg#163).
const CELL_LINE_BREAK: &str = "<br>";

/// Push `s` into `buffer`, escaping everything that would let a spreadsheet cell
/// break out of its markdown table cell.
///
/// Deliberately *not* routed through the shared `rendering::common` table
/// renderer: this path streams a whole sheet (heading, table, truncation notice)
/// into one buffer rather than rendering a `&[Vec<String>]` grid, and
/// spreadsheet text is literal, so a `\` typed into a cell is doubled to survive
/// markdown unescaping. The shared renderer must not double backslashes because
/// its inputs (DOCX, HTML, PDF) already carry markdown-significant text.
///
/// Escapes `|`, doubles `\`, and turns any line break into [`CELL_LINE_BREAK`].
#[inline]
fn escape_markdown_into(buffer: &mut String, s: &str) {
    if !s.bytes().any(|b| matches!(b, b'|' | b'\\' | b'\n' | b'\r')) {
        buffer.push_str(s);
        return;
    }
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '|' => buffer.push_str("\\|"),
            '\\' => buffer.push_str("\\\\"),
            '\r' => {
                // Consume the LF of a CRLF pair so it yields one break, not two.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                buffer.push_str(CELL_LINE_BREAK);
            }
            '\n' => buffer.push_str(CELL_LINE_BREAK),
            _ => buffer.push(ch),
        }
    }
}

/// Render a zero-based column index as spreadsheet column letters, e.g.
/// `0 -> "A"`, `25 -> "Z"`, `26 -> "AA"`.
fn column_letters(mut col: u32) -> String {
    let mut letters = Vec::new();
    loop {
        let rem = (col % 26) as u8;
        letters.push(b'A' + rem);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    letters.reverse();
    // SAFETY-free: every byte pushed above is in b'A'..=b'Z', which is valid UTF-8. ~keep
    String::from_utf8(letters).unwrap_or_default()
}

/// Render a zero-based `(row, col)` position as an A1-style cell reference
/// relative to the enumerated range (matching the convention already used by
/// `extractors::excel::scan_for_dde_warnings`, which reports positions
/// relative to the extracted `table_cells` grid rather than absolute sheet
/// coordinates).
fn cell_reference(row: u32, col: u32) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

/// Collect non-empty formulas from a sheet as `"<cell_ref>=<formula>"` entries
/// joined by `"; "`. Works across every calamine `Reader` (xlsx/xls/xlsb/ods)
/// since `worksheet_formula` is part of the shared trait. Returns `None` when
/// the sheet has no formulas or the backend cannot report them.
fn collect_sheet_formulas<RS, R>(workbook: &mut R, sheet_name: &str) -> Option<String>
where
    RS: Read + Seek,
    R: Reader<RS>,
{
    let range = workbook.worksheet_formula(sheet_name).ok()?;
    let mut entries = Vec::new();

    'outer: for (row_idx, row) in range.rows().enumerate() {
        for (col_idx, formula) in row.iter().enumerate() {
            if formula.is_empty() {
                continue;
            }
            entries.push(format!(
                "{}={}",
                cell_reference(row_idx as u32, col_idx as u32),
                formula
            ));
            if entries.len() >= MAX_METADATA_ENTRIES_PER_SHEET {
                break 'outer;
            }
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries.join("; "))
    }
}

/// Collect hyperlinks from an XLSX sheet as `"<anchor>=<target>"` entries
/// joined by `"; "`. XLSX-only: hyperlink relationships are not part of the
/// shared calamine `Reader` trait. Returns `None` when the sheet has no
/// hyperlinks or the relationships cannot be read.
fn collect_sheet_hyperlinks<RS: Read + Seek>(workbook: &mut calamine::Xlsx<RS>, sheet_name: &str) -> Option<String> {
    let hyperlinks = workbook.hyperlinks_by_sheet_name(sheet_name).ok()?;
    if hyperlinks.is_empty() {
        return None;
    }

    let entries: Vec<String> = hyperlinks
        .iter()
        .take(MAX_METADATA_ENTRIES_PER_SHEET)
        .map(|link| {
            let target = link.target.as_deref().or(link.location.as_deref()).unwrap_or("");
            let anchor = cell_reference(link.range.start.0, link.range.start.1);
            format!("{anchor}={target}")
        })
        .collect();

    Some(entries.join("; "))
}

fn extract_metadata<RS, R>(
    workbook: &R,
    sheet_names: &[String],
    office_metadata: Option<HashMap<String, String>>,
) -> HashMap<String, String>
where
    RS: std::io::Read + std::io::Seek,
    R: Reader<RS>,
{
    let mut metadata = HashMap::with_capacity(4);

    let sheet_count = sheet_names.len();
    metadata.insert("sheet_count".to_owned(), sheet_count.to_string());

    let sheet_names_str = if sheet_count <= 5 {
        sheet_names.join(", ")
    } else {
        let mut result = String::with_capacity(100);
        for (i, name) in sheet_names.iter().take(5).enumerate() {
            if i > 0 {
                result.push_str(", ");
            }
            result.push_str(name);
        }
        write!(result, ", ... ({} total)", sheet_count).unwrap();
        result
    };
    metadata.insert("sheet_names".to_owned(), sheet_names_str);

    let _workbook_metadata = workbook.metadata();

    let defined_names = workbook.defined_names();
    if !defined_names.is_empty() {
        let joined = defined_names
            .iter()
            .map(|(name, formula)| format!("{name}={formula}"))
            .collect::<Vec<_>>()
            .join("; ");
        metadata.insert("defined_names".to_owned(), joined);
    }

    let hidden_sheets: Vec<&str> = workbook
        .sheets_metadata()
        .iter()
        .filter(|sheet| !matches!(sheet.visible, SheetVisible::Visible))
        .map(|sheet| sheet.name.as_str())
        .collect();
    if !hidden_sheets.is_empty() {
        metadata.insert("hidden_sheets".to_owned(), hidden_sheets.join(", "));
    }

    if let Some(office_meta) = office_metadata {
        for (key, value) in office_meta {
            metadata.insert(key, value);
        }
    }

    metadata
}

/// Convert an Excel workbook to plain text (space-separated cells, one row per line).
///
/// Each sheet is separated by a blank line. Sheet names are included as headers.
/// This produces text suitable for quality scoring against ground truth.
#[cfg_attr(alef, alef(skip))]
pub fn excel_to_text(workbook: &ExcelWorkbook) -> String {
    let mut result = String::new();

    for (i, sheet) in workbook.sheets.iter().enumerate() {
        if i > 0 {
            result.push_str("\n\n");
        }

        if let Some(cells) = &sheet.table_cells {
            for (row_idx, row) in cells.iter().enumerate() {
                if row_idx > 0 {
                    result.push('\n');
                }
                let line: String = row
                    .iter()
                    .map(|cell| cell.trim())
                    .filter(|cell| !cell.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                result.push_str(&line);
            }
        }
    }

    result
}

/// Render all sheets in an [`ExcelWorkbook`] as a single Markdown string.
///
/// Sheets are separated by double newlines; each sheet's pre-rendered markdown
/// is trimmed of trailing whitespace before joining.
#[cfg_attr(alef, alef(skip))]
pub fn excel_to_markdown(workbook: &ExcelWorkbook) -> String {
    let total_capacity: usize = workbook.sheets.iter().map(|sheet| sheet.markdown.len() + 2).sum();

    let mut result = String::with_capacity(total_capacity);

    for (i, sheet) in workbook.sheets.iter().enumerate() {
        if i > 0 {
            result.push_str("\n\n");
        }
        let sheet_content = sheet.markdown.trim_end();
        result.push_str(sheet_content);
    }

    result
}

#[cfg(feature = "office")]
fn extract_xlsx_office_metadata_from_file(file_path: &str) -> Result<HashMap<String, String>> {
    use std::fs::File;
    use zip::ZipArchive;

    // OSError/RuntimeError must bubble up - system errors need user reports ~keep
    let file = File::open(file_path)?;

    let mut archive =
        ZipArchive::new(file).map_err(|e| XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?;

    extract_xlsx_office_metadata_from_archive(&mut archive)
}

#[cfg(feature = "office")]
fn extract_xlsx_office_metadata_from_bytes(data: &[u8]) -> Result<HashMap<String, String>> {
    use zip::ZipArchive;

    let cursor = Cursor::new(data);
    let mut archive =
        ZipArchive::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?;

    extract_xlsx_office_metadata_from_archive(&mut archive)
}

/// Read ODS document metadata from the spreadsheet's `meta.xml`.
///
/// An `.ods` is an ODF package, not an OOXML one: its metadata lives in `meta.xml`
/// under the ODF namespaces rather than in `docProps/core.xml`, so the OOXML reader
/// finds nothing and every ODS came back with no title, author or dates at all. ODT
/// and ODP already read this exact part through the same helper (#102).
///
/// The keys match [`extract_xlsx_office_metadata_from_archive`]'s so both spreadsheet
/// families populate `Metadata` identically downstream.
#[cfg(feature = "office")]
fn extract_ods_office_metadata_from_archive<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<HashMap<String, String>> {
    let properties = crate::extraction::office_metadata::extract_odt_properties(archive)?;
    let mut metadata = HashMap::new();

    let mut insert = |key: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            metadata.insert(key.to_string(), value);
        }
    };

    insert("title", properties.title);
    insert("subject", properties.subject);
    insert("keywords", properties.keywords);
    insert("description", properties.description);
    insert("language", properties.language);
    insert("revision", properties.editing_cycles);
    insert("created_at", properties.creation_date);
    insert("modified_at", properties.date);

    // ODF splits authorship the other way round from OOXML: `meta:initial-creator` is
    // who created the document and `dc:creator` is who touched it last, whereas OOXML's
    // `dc:creator` is the original author. Map by role, not by tag name, and fall back to
    // `dc:creator` for the author when a producer omits `meta:initial-creator`.
    let author = properties.initial_creator.or_else(|| properties.creator.clone());
    insert("creator", author.clone());
    insert("created_by", author);
    insert("modified_by", properties.creator);

    Ok(metadata)
}

#[cfg(feature = "office")]
fn extract_ods_office_metadata_from_file(file_path: &str) -> Result<HashMap<String, String>> {
    use std::fs::File;
    use zip::ZipArchive;

    // OSError/RuntimeError must bubble up - system errors need user reports ~keep
    let file = File::open(file_path)?;

    let mut archive =
        ZipArchive::new(file).map_err(|e| XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?;

    extract_ods_office_metadata_from_archive(&mut archive)
}

#[cfg(feature = "office")]
fn extract_ods_office_metadata_from_bytes(data: &[u8]) -> Result<HashMap<String, String>> {
    use zip::ZipArchive;

    let cursor = Cursor::new(data);
    let mut archive =
        ZipArchive::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?;

    extract_ods_office_metadata_from_archive(&mut archive)
}

#[cfg(feature = "office")]
fn extract_xlsx_office_metadata_from_archive<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();

    if let Ok(core) = extract_core_properties(archive) {
        if let Some(title) = core.title {
            metadata.insert("title".to_string(), title);
        }
        if let Some(creator) = core.creator {
            metadata.insert("creator".to_string(), creator.clone());
            metadata.insert("created_by".to_string(), creator);
        }
        if let Some(subject) = core.subject {
            metadata.insert("subject".to_string(), subject);
        }
        if let Some(keywords) = core.keywords {
            metadata.insert("keywords".to_string(), keywords);
        }
        if let Some(description) = core.description {
            metadata.insert("description".to_string(), description);
        }
        if let Some(modified_by) = core.last_modified_by {
            metadata.insert("modified_by".to_string(), modified_by);
        }
        if let Some(created) = core.created {
            metadata.insert("created_at".to_string(), created);
        }
        if let Some(modified) = core.modified {
            metadata.insert("modified_at".to_string(), modified);
        }
        if let Some(revision) = core.revision {
            metadata.insert("revision".to_string(), revision);
        }
        if let Some(category) = core.category {
            metadata.insert("category".to_string(), category);
        }
        if let Some(content_status) = core.content_status {
            metadata.insert("content_status".to_string(), content_status);
        }
        if let Some(language) = core.language {
            metadata.insert("language".to_string(), language);
        }
    }

    if let Ok(app) = extract_xlsx_app_properties(archive) {
        if !app.worksheet_names.is_empty() {
            metadata.insert("worksheet_names".to_string(), app.worksheet_names.join(", "));
        }
        if let Some(company) = app.company {
            metadata.insert("organization".to_string(), company);
        }
        if let Some(application) = app.application {
            metadata.insert("application".to_string(), application);
        }
        if let Some(app_version) = app.app_version {
            metadata.insert("application_version".to_string(), app_version);
        }
        // #230: surface the raw DocSecurity integer plus its decoded ECMA-376 flags.
        // `XlsxAppProperties` never reaches `FormatMetadata::Excel`, so without this the
        // workbook's protection state was discarded entirely.
        if let Some(raw) = app.doc_security {
            metadata.insert(DOC_SECURITY_KEY.to_string(), raw.to_string());
            for (key, value) in decode_doc_security_flags(raw) {
                metadata.insert(key.to_string(), value.to_string());
            }
        }
    }

    if let Ok(custom) = extract_custom_properties(archive) {
        for (key, value) in custom {
            let value_str = match value {
                Value::String(s) => s,
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => "null".to_string(),
                Value::Array(_) | Value::Object(_) => value.to_string(),
            };
            metadata.insert(format!("custom_{}", key), value_str);
        }
    }

    Ok(metadata)
}

/// Extract revision headers from an in-memory `.xlsx`/`.xlsm`/`.xltm` blob.
///
/// Returns `None` when `xl/revisions/revisionHeaders.xml` is absent (the
/// common case for modern files that don't use legacy shared-workbook mode).
/// Returns `Some(vec![])` when the file exists but contains no `<header>`
/// elements. On any parse error the function logs a warning and returns `None`
/// so that the rest of extraction succeeds.
fn extract_xlsx_revisions_from_bytes(data: &[u8]) -> Option<Vec<DocumentRevision>> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;
    extract_xlsx_revisions_from_archive(&mut archive)
}

/// Extract revision headers from an `.xlsx`/`.xlsm`/`.xltm` file on disk.
///
/// Same semantics as [`extract_xlsx_revisions_from_bytes`].
fn extract_xlsx_revisions_from_file(file_path: &str) -> Option<Vec<DocumentRevision>> {
    let file = std::fs::File::open(file_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    extract_xlsx_revisions_from_archive(&mut archive)
}

/// Core revision-header parser shared by the file and bytes paths.
fn extract_xlsx_revisions_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Option<Vec<DocumentRevision>> {
    const HEADERS_PATH: &str = "xl/revisions/revisionHeaders.xml";

    let xml_bytes = {
        let mut entry = match archive.by_name(HEADERS_PATH) {
            Ok(e) => e,
            Err(zip::result::ZipError::FileNotFound) => return None,
            Err(e) => {
                tracing::warn!(
                    path = HEADERS_PATH,
                    error = %e,
                    "failed to open xl/revisions/revisionHeaders.xml"
                );
                return None;
            }
        };
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() {
            return None;
        }
        buf
    };

    match parse_revision_headers_xml(&xml_bytes) {
        Ok(revisions) => Some(revisions),
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse xl/revisions/revisionHeaders.xml");
            None
        }
    }
}

/// Parse `xl/revisions/revisionHeaders.xml` and emit one `DocumentRevision`
/// per `<header>` element.
///
/// Each header carries a `guid` (→ `revision_id`), `userName` (→ `author`),
/// and `dateTime` (→ `timestamp`). `anchor` and `delta` are empty for v1;
/// per-cell log parsing (`revisionLog*.xml`) is a future follow-up.
///
/// `RevisionKind::FormatChange` is used as the closest available variant
/// because the header file does not distinguish what *kind* of changes the
/// revision contains — that information is in the per-revision log file.
fn parse_revision_headers_xml(xml_bytes: &[u8]) -> Result<Vec<DocumentRevision>> {
    let xml_str = crate::text::utf8_validation::from_utf8(xml_bytes)
        .map_err(|e| XbergError::parsing(format!("invalid UTF-8 in revisionHeaders.xml: {e}")))?;

    let doc = roxmltree::Document::parse(xml_str)
        .map_err(|e| XbergError::parsing(format!("failed to parse revisionHeaders.xml: {e}")))?;

    const SPREADSHEETML_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

    let mut revisions = Vec::new();
    for node in doc.descendants() {
        if !node.has_tag_name((SPREADSHEETML_NS, "header")) {
            continue;
        }
        let revision_id = node
            .attribute("guid")
            .unwrap_or("")
            .trim_matches(|c| c == '{' || c == '}')
            .to_string();
        let revision_id = if revision_id.is_empty() {
            format!("xlsx-rev-{}", revisions.len())
        } else {
            revision_id
        };
        let author = node.attribute("userName").filter(|s| !s.is_empty()).map(str::to_string);
        let timestamp = node.attribute("dateTime").filter(|s| !s.is_empty()).map(str::to_string);
        revisions.push(DocumentRevision {
            revision_id,
            author,
            timestamp,
            kind: RevisionKind::FormatChange,
            anchor: None,
            delta: RevisionDelta::default(),
        });
    }

    Ok(revisions)
}

/// Extract cell comments from an in-memory `.xlsx`/`.xlsm`/`.xltm` blob.
///
/// Returns `None` when the archive contains no `xl/comments*.xml` parts (the
/// common case: most workbooks have no cell comments) or cannot be opened as
/// a ZIP. Otherwise returns every comment found across all comment parts as
/// `"<cell_ref>: <text>"` entries joined by `"; "`.
///
/// Comments are *not* attributed to a specific sheet here: resolving which
/// `xl/comments<N>.xml` part belongs to which worksheet requires walking
/// `xl/worksheets/_rels/sheetN.xml.rels`, which is a follow-up (xberg-io/xberg#89).
/// The cell reference in each entry is still enough to locate the comment
/// inside the workbook.
fn extract_xlsx_comments_from_bytes(data: &[u8]) -> Option<String> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;
    extract_xlsx_comments_from_archive(&mut archive)
}

/// Extract cell comments from an `.xlsx`/`.xlsm`/`.xltm` file on disk. Same
/// semantics as [`extract_xlsx_comments_from_bytes`].
fn extract_xlsx_comments_from_file(file_path: &str) -> Option<String> {
    let file = std::fs::File::open(file_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    extract_xlsx_comments_from_archive(&mut archive)
}

/// Core comment-part scanner shared by the file and bytes paths.
fn extract_xlsx_comments_from_archive<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Option<String> {
    let mut comment_parts: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let name = archive.by_index(i).ok()?.name().to_string();
        if name.starts_with("xl/comments") && name.ends_with(".xml") {
            comment_parts.push(name);
        }
    }
    if comment_parts.is_empty() {
        return None;
    }
    comment_parts.sort();

    let mut entries = Vec::new();
    for part in comment_parts {
        let xml_bytes = {
            let mut entry = match archive.by_name(&part) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let mut buf = Vec::new();
            if entry.read_to_end(&mut buf).is_err() {
                continue;
            }
            buf
        };
        if let Ok(parsed) = parse_comments_xml(&xml_bytes) {
            entries.extend(parsed);
        } else {
            tracing::warn!(part = %part, "failed to parse worksheet comments part");
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries.join("; "))
    }
}

/// Parse a single `xl/comments<N>.xml` part into `"<cell_ref>: <text>"`
/// strings, one per `<comment>` element with non-empty text. A comment's text
/// may be split across multiple `<r><t>` runs (rich text); these are
/// concatenated in document order.
fn parse_comments_xml(xml_bytes: &[u8]) -> Result<Vec<String>> {
    let xml_str = crate::text::utf8_validation::from_utf8(xml_bytes)
        .map_err(|e| XbergError::parsing(format!("invalid UTF-8 in comments.xml: {e}")))?;

    let doc = roxmltree::Document::parse(xml_str)
        .map_err(|e| XbergError::parsing(format!("failed to parse comments.xml: {e}")))?;

    const SPREADSHEETML_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

    let mut entries = Vec::new();
    for node in doc.descendants() {
        if !node.has_tag_name((SPREADSHEETML_NS, "comment")) {
            continue;
        }
        let cell_ref = node.attribute("ref").unwrap_or("");
        let text: String = node
            .descendants()
            .filter(|n| n.has_tag_name((SPREADSHEETML_NS, "t")))
            .filter_map(|n| n.text())
            .collect();
        let text = text.trim();
        if !text.is_empty() {
            entries.push(format!("{cell_ref}: {text}"));
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for #102: office metadata was computed only for the OOXML
    /// spreadsheet extensions, so an `.ods` reached `ExcelWorkbook` with an empty
    /// metadata map — no title, no author, no dates — even though ODT and ODP read
    /// the very same `meta.xml` through `extract_odt_properties`.
    ///
    /// The expectations are read straight out of the fixture's own `meta.xml`.
    #[cfg(feature = "office")]
    #[test]
    fn should_read_ods_document_metadata_from_meta_xml() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/data_formats/test_01.ods");
        if !path.exists() {
            println!("Skipping: test document not found at {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path).expect("read fixture");

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".ods").expect("ODS extraction should succeed");

        assert_eq!(
            workbook.metadata.get("creator").map(String::as_str),
            Some("Peter Staar"),
            "meta:initial-creator must map to the document author; got {:?}",
            workbook.metadata
        );
        assert_eq!(
            workbook.metadata.get("modified_by").map(String::as_str),
            Some("Peter Staar"),
            "dc:creator is ODF's last-modifier"
        );
        assert_eq!(
            workbook.metadata.get("created_at").map(String::as_str),
            Some("2024-11-16T05:17:41")
        );
        assert_eq!(
            workbook.metadata.get("modified_at").map(String::as_str),
            Some("2025-01-24T13:18:51")
        );
    }

    #[test]
    fn test_format_cell_to_string_basic() {
        assert_eq!(format_cell_to_string(&Data::Empty), "");
        assert_eq!(format_cell_to_string(&Data::String("test".to_owned())), "test");
        assert_eq!(format_cell_to_string(&Data::Float(42.0)), "42");
        assert_eq!(format_cell_to_string(&Data::Int(100)), "100");
        assert_eq!(format_cell_to_string(&Data::Bool(true)), "true");
    }

    #[test]
    fn test_escape_markdown_into() {
        let mut buffer = String::with_capacity(50);

        escape_markdown_into(&mut buffer, "normal text");
        assert_eq!(buffer, "normal text");

        buffer.clear();
        escape_markdown_into(&mut buffer, "text|with|pipes");
        assert_eq!(buffer, "text\\|with\\|pipes");

        buffer.clear();
        escape_markdown_into(&mut buffer, "back\\slash");
        assert_eq!(buffer, "back\\\\slash");
    }

    #[test]
    fn test_capacity_optimization() {
        let buffer = String::with_capacity(100);
        assert!(buffer.capacity() >= 100);
    }

    #[test]
    fn test_format_cell_value_datetime() {
        use calamine::{ExcelDateTime, ExcelDateTimeType};
        let dt = Data::DateTime(ExcelDateTime::new(49353.5, ExcelDateTimeType::DateTime, false));
        let result = format_cell_to_string(&dt);
        assert!(!result.is_empty());
        assert!(result.contains('-'), "Expected datetime string, got: {}", result);
    }

    #[test]
    fn test_format_cell_value_error() {
        use calamine::CellErrorType;
        let result = format_cell_to_string(&Data::Error(CellErrorType::Div0));
        assert!(result.contains("#ERR"));
    }

    #[test]
    fn test_format_cell_value_datetime_iso() {
        let result = format_cell_to_string(&Data::DateTimeIso("2024-01-01T10:30:00".to_owned()));
        assert_eq!(result, "2024-01-01T10:30:00");
    }

    #[test]
    fn test_format_cell_value_duration_iso() {
        let result = format_cell_to_string(&Data::DurationIso("PT1H30M".to_owned()));
        assert_eq!(result, "DURATION: PT1H30M");
    }

    #[test]
    fn test_escape_markdown_combined() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "text|with|pipes\\and\\slashes");
        assert_eq!(buffer, "text\\|with\\|pipes\\\\and\\\\slashes");
    }

    #[test]
    fn test_escape_markdown_no_special_chars() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "plain text");
        assert_eq!(buffer, "plain text");
    }

    /// xberg-io/xberg#163: a spreadsheet cell may contain a hard line break
    /// (Alt+Enter). Emitted raw it ends the markdown table row, so the tail of
    /// the cell becomes a malformed extra row.
    #[test]
    fn should_replace_cell_line_break_with_break_tag() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "line1\nline2");
        assert_eq!(buffer, "line1<br>line2");
    }

    #[test]
    fn should_collapse_cell_crlf_to_a_single_break_tag() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "line1\r\nline2");
        assert_eq!(buffer, "line1<br>line2");
    }

    #[test]
    fn should_replace_lone_carriage_return_with_break_tag() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "line1\rline2");
        assert_eq!(buffer, "line1<br>line2");
    }

    #[test]
    fn should_escape_pipes_backslashes_and_line_breaks_together() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "a|b\\c\nd");
        assert_eq!(buffer, "a\\|b\\\\c<br>d");
    }

    #[test]
    fn test_process_sheet_empty() {
        let range: Range<Data> = Range::empty();
        let mut warnings = Vec::new();
        let sheet = process_sheet("EmptySheet", &range, &mut warnings);

        assert_eq!(sheet.name, "EmptySheet");
        assert_eq!(sheet.row_count, 0);
        assert_eq!(sheet.col_count, 0);
        assert_eq!(sheet.cell_count, 0);
        assert!(sheet.markdown.contains("Empty sheet"));
    }

    #[test]
    fn test_process_sheet_single_cell() {
        let mut range: Range<Data> = Range::new((0, 0), (0, 0));
        range.set_value((0, 0), Data::String("Single Cell".to_owned()));

        let mut warnings = Vec::new();
        let sheet = process_sheet("Sheet1", &range, &mut warnings);

        assert_eq!(sheet.name, "Sheet1");
        assert_eq!(sheet.row_count, 1);
        assert_eq!(sheet.col_count, 1);
        assert_eq!(sheet.cell_count, 1);
        assert!(sheet.markdown.contains("Single Cell"));
    }

    #[test]
    fn test_process_sheet_with_data() {
        let mut range: Range<Data> = Range::new((0, 0), (2, 1));
        range.set_value((0, 0), Data::String("Name".to_owned()));
        range.set_value((0, 1), Data::String("Age".to_owned()));
        range.set_value((1, 0), Data::String("Alice".to_owned()));
        range.set_value((1, 1), Data::Int(30));
        range.set_value((2, 0), Data::String("Bob".to_owned()));
        range.set_value((2, 1), Data::Int(25));

        let mut warnings = Vec::new();
        let sheet = process_sheet("People", &range, &mut warnings);

        assert_eq!(sheet.name, "People");
        assert_eq!(sheet.row_count, 3);
        assert_eq!(sheet.col_count, 2);
        assert!(sheet.markdown.contains("Name"));
        assert!(sheet.markdown.contains("Age"));
        assert!(sheet.markdown.contains("Alice"));
        assert!(sheet.markdown.contains("30"));
    }

    #[test]
    fn test_generate_markdown_and_cells_empty() {
        let range: Range<Data> = Range::empty();
        let mut warnings = Vec::new();
        let (markdown, cells) = generate_markdown_and_cells("Test", &range, 100, &mut warnings);

        assert!(markdown.contains("## Test"));
        assert!(cells.is_empty());
    }

    #[test]
    fn test_generate_markdown_and_cells_with_data() {
        let mut range: Range<Data> = Range::new((0, 0), (1, 2));
        range.set_value((0, 0), Data::String("Col1".to_owned()));
        range.set_value((0, 1), Data::String("Col2".to_owned()));
        range.set_value((0, 2), Data::String("Col3".to_owned()));
        range.set_value((1, 0), Data::String("A".to_owned()));
        range.set_value((1, 1), Data::String("B".to_owned()));
        range.set_value((1, 2), Data::String("C".to_owned()));

        let mut warnings = Vec::new();
        let (markdown, cells) = generate_markdown_and_cells("Sheet1", &range, 200, &mut warnings);

        assert!(markdown.contains("## Sheet1"));
        assert!(markdown.contains("Col1"));
        assert!(markdown.contains("---"));
        assert_eq!(cells.len(), 2);
    }

    #[test]
    fn test_generate_markdown_and_cells_sparse() {
        let mut range: Range<Data> = Range::new((0, 0), (2, 2));
        range.set_value((0, 0), Data::String("A".to_owned()));
        range.set_value((0, 1), Data::String("B".to_owned()));
        range.set_value((0, 2), Data::String("C".to_owned()));
        range.set_value((1, 0), Data::String("X".to_owned()));
        range.set_value((1, 2), Data::String("Z".to_owned()));

        let mut warnings = Vec::new();
        let (markdown, cells) = generate_markdown_and_cells("Sparse", &range, 200, &mut warnings);

        assert!(markdown.contains("X"));
        assert!(markdown.contains("Z"));
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn test_format_cell_value_float_integer() {
        let result = format_cell_to_string(&Data::Float(100.0));
        assert_eq!(result, "100");
    }

    #[test]
    fn test_format_cell_value_float_decimal() {
        let result = format_cell_to_string(&Data::Float(12.3456));
        assert_eq!(result, "12.3456");
    }

    #[test]
    fn test_format_cell_value_bool_false() {
        let result = format_cell_to_string(&Data::Bool(false));
        assert_eq!(result, "false");
    }

    #[test]
    fn test_format_cell_escape_pipe() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "value|with|pipes");
        assert_eq!(buffer, "value\\|with\\|pipes");
    }

    #[test]
    fn test_format_cell_escape_backslash() {
        let mut buffer = String::new();
        escape_markdown_into(&mut buffer, "path\\to\\file");
        assert_eq!(buffer, "path\\\\to\\\\file");
    }

    #[test]
    fn test_markdown_table_structure() {
        let mut range: Range<Data> = Range::new((0, 0), (2, 1));
        range.set_value((0, 0), Data::String("H1".to_owned()));
        range.set_value((0, 1), Data::String("H2".to_owned()));
        range.set_value((1, 0), Data::String("A".to_owned()));
        range.set_value((1, 1), Data::String("B".to_owned()));

        let mut warnings = Vec::new();
        let (markdown, _cells) = generate_markdown_and_cells("Test", &range, 100, &mut warnings);

        let lines: Vec<&str> = markdown.lines().collect();
        assert!(lines[0].contains("## Test"));
        assert!(lines[2].starts_with("| "));
        assert!(lines[3].contains("---"));
        assert!(lines[4].starts_with("| "));
    }

    #[test]
    fn test_process_sheet_metadata() {
        let mut range: Range<Data> = Range::new((0, 0), (9, 4));
        for row in 0..10 {
            for col in 0..5 {
                range.set_value((row, col), Data::String(format!("R{}C{}", row, col)));
            }
        }

        let mut warnings = Vec::new();
        let sheet = process_sheet("Data", &range, &mut warnings);

        assert_eq!(sheet.row_count, 10);
        assert_eq!(sheet.col_count, 5);
        assert_eq!(sheet.cell_count, 50);
    }

    fn make_xlsx_with_worksheet(worksheet_xml: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/xl/workbook.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
            )
            .unwrap();

            zip.start_file("xl/workbook.xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
            )
            .unwrap();

            zip.start_file("xl/_rels/workbook.xml.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"
    Target="worksheets/sheet1.xml"/>
</Relationships>"#,
            )
            .unwrap();

            zip.start_file("xl/worksheets/sheet1.xml", options).unwrap();
            zip.write_all(worksheet_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn should_preserve_dense_xlsx_output_after_position_only_preflight() {
        let bytes = make_xlsx_with_worksheet(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:B2"/>
  <sheetData>
    <row r="1">
      <c r="A1" t="inlineStr"><is><t>Header</t></is></c>
      <c r="B1" t="inlineStr"><is><t>Value</t></is></c>
    </row>
    <row r="2">
      <c r="A2" t="inlineStr"><is><t>Alpha</t></is></c>
      <c r="B2" t="inlineStr"><is><t>Beta</t></is></c>
    </row>
  </sheetData>
</worksheet>"#,
        );
        let mut workbook = calamine::Xlsx::new(Cursor::new(bytes.as_slice())).unwrap();

        assert_eq!(
            classify_xlsx_sheet_shape(&mut workbook, "Sheet1").unwrap(),
            XlsxSheetShape::Dense
        );
        let mut warnings = Vec::new();
        let sheet = process_xlsx_sheet_safe(&mut workbook, "Sheet1", &mut warnings).unwrap();

        assert_eq!(sheet.row_count, 2);
        assert_eq!(sheet.col_count, 2);
        assert_eq!(sheet.cell_count, 4);
        assert_eq!(
            sheet.table_cells,
            Some(vec![
                vec!["Header".to_string(), "Value".to_string()],
                vec!["Alpha".to_string(), "Beta".to_string()],
            ])
        );
        assert_eq!(
            sheet.markdown,
            "## Sheet1\n\n| Header | Value |\n| --- | --- |\n| Alpha | Beta |\n"
        );
    }

    #[test]
    fn should_preserve_pathological_sparse_xlsx_output_after_reopening_reader() {
        let bytes = make_xlsx_with_worksheet(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:XFD1048575"/>
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t>Start</t></is></c></row>
    <row r="1048575"><c r="XFD1048575" t="inlineStr"><is><t>End</t></is></c></row>
  </sheetData>
</worksheet>"#,
        );
        let mut workbook = calamine::Xlsx::new(Cursor::new(bytes.as_slice())).unwrap();

        assert_eq!(
            classify_xlsx_sheet_shape(&mut workbook, "Sheet1").unwrap(),
            XlsxSheetShape::Sparse
        );
        let mut warnings = Vec::new();
        let sheet = process_xlsx_sheet_safe(&mut workbook, "Sheet1", &mut warnings).unwrap();

        assert_eq!(sheet.row_count, 1_048_575);
        assert_eq!(sheet.col_count, 16_384);
        assert_eq!(sheet.cell_count, 2);
        assert_eq!(
            sheet.table_cells,
            Some(vec![
                vec!["Start".to_string(), String::new()],
                vec![String::new(), "End".to_string()],
            ])
        );
        assert_eq!(sheet.markdown, "## Sheet1\n\n| Start |  |\n| --- | --- |\n|  | End |\n");
    }

    /// Build a minimal in-memory `.xlsx` zip that contains a synthetic
    /// `xl/revisions/revisionHeaders.xml` with the given `<header>` elements.
    ///
    /// `headers` is a slice of `(guid, user_name, date_time)` tuples.
    fn make_xlsx_with_revision_headers(headers: &[(&str, &str, &str)]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let opts = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels"
    ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/xl/workbook.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("_rels/.rels", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
            )
            .unwrap();

            zip.start_file("xl/workbook.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#,
            )
            .unwrap();

            zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"
    Target="worksheets/sheet1.xml"/>
</Relationships>"#,
            )
            .unwrap();

            zip.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData/>
</worksheet>"#,
            )
            .unwrap();

            let mut headers_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<headers xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            );
            for (guid, user, dt) in headers {
                use std::fmt::Write as FmtWrite;
                let _ = write!(
                    headers_xml,
                    r#"<header guid="{{{guid}}}" dateTime="{dt}" userName="{user}" maxSheetId="1"/>"#,
                );
            }
            headers_xml.push_str("\n</headers>");

            zip.start_file("xl/revisions/revisionHeaders.xml", opts).unwrap();
            zip.write_all(headers_xml.as_bytes()).unwrap();

            let _ = zip.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn should_return_none_revisions_when_xl_revisions_absent() {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let opts = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("xl/workbook.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets/></workbook>"#).unwrap();

            let _ = zip.finish().unwrap();
        }

        let result = extract_xlsx_revisions_from_bytes(&buffer);
        assert!(result.is_none(), "expected None when xl/revisions/ is absent");
    }

    #[test]
    fn should_parse_two_revision_headers_with_correct_fields() {
        let xlsx = make_xlsx_with_revision_headers(&[
            ("11111111-2222-3333-4444-555555555555", "Alice", "2024-01-15T09:00:00Z"),
            ("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE", "Bob", "2024-01-16T14:30:00Z"),
        ]);

        let revisions = extract_xlsx_revisions_from_bytes(&xlsx)
            .expect("revisions should be Some when xl/revisions/revisionHeaders.xml is present");

        assert_eq!(revisions.len(), 2, "expected 2 revisions from 2 headers");

        assert_eq!(
            revisions[0].revision_id, "11111111-2222-3333-4444-555555555555",
            "guid should be stored without braces"
        );
        assert_eq!(revisions[0].author.as_deref(), Some("Alice"));
        assert_eq!(revisions[0].timestamp.as_deref(), Some("2024-01-15T09:00:00Z"));
        assert!(
            matches!(revisions[0].kind, RevisionKind::FormatChange),
            "kind should be FormatChange for v1 headers"
        );
        assert!(revisions[0].anchor.is_none(), "anchor should be None for v1");
        assert!(
            revisions[0].delta.content.is_empty(),
            "delta.content should be empty for v1"
        );

        assert_eq!(revisions[1].revision_id, "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE");
        assert_eq!(revisions[1].author.as_deref(), Some("Bob"));
        assert_eq!(revisions[1].timestamp.as_deref(), Some("2024-01-16T14:30:00Z"));
    }

    #[test]
    fn should_return_some_empty_vec_when_headers_xml_exists_but_has_no_header_elements() {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let opts = SimpleFileOptions::default();
            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>").unwrap();

            zip.start_file("xl/revisions/revisionHeaders.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<headers xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>
"#,
            )
            .unwrap();
            let _ = zip.finish().unwrap();
        }

        let revisions = extract_xlsx_revisions_from_bytes(&buffer).expect("revisions should be Some when file exists");
        assert!(revisions.is_empty(), "expected empty vec when no <header> elements");
    }

    #[test]
    fn should_surface_revisions_in_full_xlsx_extraction() {
        let xlsx = make_xlsx_with_revision_headers(&[(
            "DEADBEEF-0000-0000-0000-000000000001",
            "Carol",
            "2024-03-01T12:00:00Z",
        )]);

        let (workbook, _warnings) = read_excel_bytes(&xlsx, ".xlsx").expect("should parse workbook");
        let revisions = workbook
            .revisions
            .as_ref()
            .expect("revisions should be Some after full extraction");
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].author.as_deref(), Some("Carol"));
    }

    /// Build a minimal in-memory `.xlsx` zip from a caller-supplied `<workbook>`
    /// inner body (the `<sheets>`/`<definedNames>` elements) and workbook-rels
    /// relationships, plus any additional zip parts (worksheet XML, worksheet
    /// relationships, comment parts). `[Content_Types].xml`/`_rels/.rels` mirror
    /// `make_xlsx_with_worksheet`'s already-proven-working shape.
    fn make_xlsx(workbook_body: &str, workbook_rels_body: &str, extra_parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/xl/workbook.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml"
    ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
    Target="xl/workbook.xml"/>
</Relationships>"#,
            )
            .unwrap();

            zip.start_file("xl/workbook.xml", options).unwrap();
            let workbook_xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
                 xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
                 {workbook_body}</workbook>"
            );
            zip.write_all(workbook_xml.as_bytes()).unwrap();

            zip.start_file("xl/_rels/workbook.xml.rels", options).unwrap();
            let rels_xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 {workbook_rels_body}</Relationships>"
            );
            zip.write_all(rels_xml.as_bytes()).unwrap();

            for (path, data) in extra_parts {
                zip.start_file(*path, options).unwrap();
                zip.write_all(data).unwrap();
            }

            zip.finish().unwrap();
        }
        buffer
    }

    const WORKSHEET_REL: &str = r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>"#;

    /// xberg-io/xberg#84: a sheet declared in `workbook.xml`/`workbook.xml.rels`
    /// whose worksheet part is missing from the zip must not silently vanish —
    /// the workbook must come back with the readable sheets plus a warning
    /// naming the one that failed.
    #[test]
    fn should_warn_when_xlsx_sheet_part_is_missing() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t>Present</t></is></c></row>
  </sheetData>
</worksheet>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Good" sheetId="1" r:id="rId1"/><sheet name="Missing" sheetId="2" r:id="rId2"/></sheets>"#,
            &format!(
                "{WORKSHEET_REL}\
                 <Relationship Id=\"rId2\" \
                 Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
                 Target=\"worksheets/sheet2.xml\"/>"
            ),
            &[("xl/worksheets/sheet1.xml", sheet1_xml.to_vec())],
        );

        let (workbook, warnings) =
            read_excel_bytes(&bytes, ".xlsx").expect("a workbook with one missing sheet part must still parse");

        assert_eq!(workbook.sheets.len(), 1, "only the readable sheet must be kept");
        assert_eq!(workbook.sheets[0].name, "Good");

        let sheet_warnings: Vec<_> = warnings.iter().filter(|w| w.source == "excel").collect();
        assert_eq!(
            sheet_warnings.len(),
            1,
            "expected exactly one warning for the unreadable sheet, got: {warnings:?}"
        );
        assert!(
            sheet_warnings[0].message.contains("Missing"),
            "warning must name the failed sheet: {}",
            sheet_warnings[0].message
        );
    }

    /// xberg-io/xberg#222: a sheet whose declared dimensions vastly exceed its
    /// actual data is skipped to avoid an OOM allocation; that skip must be
    /// visible to the caller as a warning naming the sheet and the cap, not
    /// only as text buried in the sheet's own markdown.
    #[test]
    fn should_warn_when_sheet_declared_dimensions_greatly_exceed_actual_data() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:A100002"/>
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t>Start</t></is></c></row>
    <row r="100002"><c r="A100002" t="inlineStr"><is><t>End</t></is></c></row>
  </sheetData>
</worksheet>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Huge" sheetId="1" r:id="rId1"/></sheets>"#,
            WORKSHEET_REL,
            &[("xl/worksheets/sheet1.xml", sheet1_xml.to_vec())],
        );

        let (workbook, warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(workbook.sheets.len(), 1);
        assert!(
            workbook.sheets[0].markdown.contains("Skipping to prevent OOM"),
            "sheet content must note the OOM-avoidance skip: {}",
            workbook.sheets[0].markdown
        );

        let truncation_warnings: Vec<_> = warnings
            .iter()
            .filter(|w| w.source == "excel" && w.message.contains("declared"))
            .collect();
        assert_eq!(
            truncation_warnings.len(),
            1,
            "expected exactly one 'declared dimensions' warning, got: {warnings:?}"
        );
        assert!(truncation_warnings[0].message.contains("Huge"));
        assert!(truncation_warnings[0].message.contains("100002"));
    }

    /// xberg-io/xberg#222: a sparse sheet whose non-empty rows/columns exceed the
    /// internal display cap must warn, naming the sheet and the actual vs.
    /// displayed size, in addition to the existing in-markdown truncation notice.
    #[test]
    fn should_warn_when_sparse_sheet_output_is_truncated() {
        let cells: Vec<((u32, u32), Data)> = (0..1500u32)
            .map(|row| ((row, 0u32), Data::String(format!("v{row}"))))
            .collect();

        let mut warnings = Vec::new();
        let sheet = process_sparse_sheet_from_cells("Big", cells, 0, 1499, 0, 0, &mut warnings)
            .expect("sparse sheet construction must succeed");

        assert_eq!(sheet.row_count, 1500);
        assert!(
            sheet.markdown.contains("Truncated: showing 1000x1"),
            "markdown must retain the existing truncation notice: {}",
            sheet.markdown
        );

        let truncation_warnings: Vec<_> = warnings.iter().filter(|w| w.source == "excel").collect();
        assert_eq!(
            truncation_warnings.len(),
            1,
            "expected exactly one truncation warning, got: {warnings:?}"
        );
        assert!(truncation_warnings[0].message.contains("Big"));
        assert!(truncation_warnings[0].message.contains("1000x1"));
    }

    /// xberg-io/xberg#119: a hidden or very-hidden sheet must be flagged in
    /// workbook metadata so a caller can tell it apart from a visible sheet;
    /// its content is still extracted, not dropped.
    #[test]
    fn should_record_hidden_sheets_in_workbook_metadata() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Visible</t></is></c></row></sheetData>
</worksheet>"#;
        let sheet2_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Secret</t></is></c></row></sheetData>
</worksheet>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Visible" sheetId="1" r:id="rId1"/><sheet name="Hidden" sheetId="2" state="hidden" r:id="rId2"/></sheets>"#,
            &format!(
                "{WORKSHEET_REL}\
                 <Relationship Id=\"rId2\" \
                 Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
                 Target=\"worksheets/sheet2.xml\"/>"
            ),
            &[
                ("xl/worksheets/sheet1.xml", sheet1_xml.to_vec()),
                ("xl/worksheets/sheet2.xml", sheet2_xml.to_vec()),
            ],
        );

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(workbook.sheets.len(), 2, "hidden sheet content must still be extracted");
        assert_eq!(
            workbook.metadata.get("hidden_sheets").map(String::as_str),
            Some("Hidden")
        );
        let hidden_sheet = workbook.sheets.iter().find(|s| s.name == "Hidden").unwrap();
        assert!(hidden_sheet.markdown.contains("Secret"));
    }

    /// xberg-io/xberg#89 / #119: non-empty cell formulas must be recoverable from
    /// the workbook, not silently discarded once calamine resolves them to a
    /// cached value.
    #[test]
    fn should_record_sheet_formulas_in_workbook_metadata() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1:B1"/>
  <sheetData>
    <row r="1">
      <c r="A1"><f>SUM(B1:B1)</f><v>5</v></c>
      <c r="B1"><v>5</v></c>
    </row>
  </sheetData>
</worksheet>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Calc" sheetId="1" r:id="rId1"/></sheets>"#,
            WORKSHEET_REL,
            &[("xl/worksheets/sheet1.xml", sheet1_xml.to_vec())],
        );

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(
            workbook.metadata.get("formulas_Calc").map(String::as_str),
            Some("A1=SUM(B1:B1)")
        );
    }

    /// xberg-io/xberg#89: cell hyperlinks must be recoverable, not dropped —
    /// calamine 0.36 exposes them via `hyperlinks_by_sheet_name`, superseding the
    /// "not accessible through the crate" limitation this module used to document.
    #[test]
    fn should_record_sheet_hyperlinks_in_workbook_metadata() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
           xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Visit</t></is></c></row></sheetData>
  <hyperlinks><hyperlink ref="A1" r:id="rId1"/></hyperlinks>
</worksheet>"#;
        let sheet1_rels = br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1"
    Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"
    Target="https://example.com/" TargetMode="External"/>
</Relationships>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Links" sheetId="1" r:id="rId1"/></sheets>"#,
            WORKSHEET_REL,
            &[
                ("xl/worksheets/sheet1.xml", sheet1_xml.to_vec()),
                ("xl/worksheets/_rels/sheet1.xml.rels", sheet1_rels.to_vec()),
            ],
        );

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(
            workbook.metadata.get("hyperlinks_Links").map(String::as_str),
            Some("A1=https://example.com/")
        );
    }

    /// xberg-io/xberg#89: workbook-level defined names must be recoverable.
    #[test]
    fn should_record_defined_names_in_workbook_metadata() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c></row></sheetData>
</worksheet>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="MyRange">Sheet1!$A$1</definedName></definedNames>"#,
            WORKSHEET_REL,
            &[("xl/worksheets/sheet1.xml", sheet1_xml.to_vec())],
        );

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(
            workbook.metadata.get("defined_names").map(String::as_str),
            Some("MyRange=Sheet1!$A$1")
        );
    }

    /// xberg-io/xberg#89: a `<comment>` element's rich-text runs must be
    /// concatenated into one string, keyed by cell reference.
    #[test]
    fn should_parse_comment_text_and_cell_ref_from_comments_xml() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <authors><author>Alice</author></authors>
  <commentList>
    <comment ref="B2" authorId="0"><text><r><t>Needs review</t></r></text></comment>
  </commentList>
</comments>"#;

        let entries = parse_comments_xml(xml).expect("comments.xml must parse");
        assert_eq!(entries, vec!["B2: Needs review".to_string()]);
    }

    /// xberg-io/xberg#89: comments must be surfaced end-to-end through
    /// `read_excel_bytes`, not just parsed in isolation.
    #[test]
    fn should_surface_comments_in_full_xlsx_extraction() {
        let sheet1_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <dimension ref="A1"/>
  <sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Value</t></is></c></row></sheetData>
</worksheet>"#;
        let comments_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <authors><author>Alice</author></authors>
  <commentList>
    <comment ref="A1" authorId="0"><text><r><t>Check this</t></r></text></comment>
  </commentList>
</comments>"#;

        let bytes = make_xlsx(
            r#"<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>"#,
            WORKSHEET_REL,
            &[
                ("xl/worksheets/sheet1.xml", sheet1_xml.to_vec()),
                ("xl/comments1.xml", comments_xml.to_vec()),
            ],
        );

        let (workbook, _warnings) = read_excel_bytes(&bytes, ".xlsx").expect("workbook must parse");

        assert_eq!(
            workbook.metadata.get("comments").map(String::as_str),
            Some("A1: Check this")
        );
    }

    /// xberg-io/xberg#103: `process_named_sheets` (the xls/xlsb/ods worksheet-range
    /// loop shared via `process_workbook`) must warn on a sheet name the backend
    /// cannot resolve, instead of silently dropping it. Uses a real on-disk `.xls`
    /// fixture plus one name that does not exist in it, so the `Err` this exercises
    /// is genuine `calamine::Reader::worksheet_range` behavior, not test-only
    /// scaffolding.
    #[test]
    fn should_warn_when_named_sheet_range_cannot_be_read() {
        let fixture_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/xls/test_excel.xls");
        let Ok(bytes) = std::fs::read(&fixture_path) else {
            eprintln!("skipping: fixture not present at {fixture_path:?}");
            return;
        };

        let mut workbook =
            calamine::Xls::new(Cursor::new(bytes.as_slice())).expect("fixture must parse as a valid XLS workbook");
        let real_names = Reader::sheet_names(&workbook);
        assert!(!real_names.is_empty(), "fixture must have at least one real sheet");

        let mut names = real_names.clone();
        names.push("Definitely-Not-A-Real-Sheet".to_owned());

        let mut warnings = Vec::new();
        let (sheets, _extra_metadata) = process_named_sheets(&mut workbook, &names, &mut warnings);

        assert_eq!(
            sheets.len(),
            real_names.len(),
            "only the real sheets should be processed"
        );
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one warning for the unresolvable sheet name, got: {warnings:?}"
        );
        assert_eq!(warnings[0].source, "excel");
        assert!(
            warnings[0].message.contains("Definitely-Not-A-Real-Sheet"),
            "warning must name the failed sheet: {}",
            warnings[0].message
        );
    }
}
