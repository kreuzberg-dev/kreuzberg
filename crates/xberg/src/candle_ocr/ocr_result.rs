//! Shared `ExtractedDocument` construction for candle-based OCR backends (issue #179).
//!
//! GLM-OCR, PaddleOCR-VL, TrOCR, and DeepSeek-OCR each independently returned a
//! bare `ExtractedDocument { content, formulas, mime_type, ..Default::default() }`,
//! silently dropping OCR metadata (`FormatMetadata::Ocr`, `ocr_used`, processed
//! image dimensions), `detected_languages`, and any GFM tables embedded in
//! `content` by GLM-OCR / PaddleOCR-VL. `sceptre_ocr::build_metadata` already
//! builds the metadata half of this correctly for Sceptre; this module is the
//! equivalent for the candle-based backends so the same logic isn't pasted a
//! fifth time (issue #179 explicitly calls out "a sixth copy is a regression").
//!
//! Not shared with `llm::vlm_ocr`: that backend lives behind the `liter-llm`
//! feature, a separate feature domain from `candle-*`, and pulling it in here
//! (or pulling this in there) would make VLM-only builds depend on candle-ocr
//! or vice versa. `vlm_ocr::process_image` builds its own (smaller) metadata
//! inline for that reason.

use std::borrow::Cow;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::core::config::OcrConfig;
use crate::types::{ExtractedDocument, FormatMetadata, Formula, Metadata, OcrMetadata, Table};

// Taken from the ungated `crate::ocr_metadata_keys` rather than `crate::ocr`, which is
// gated on `feature = "ocr"` / `"ocr-wasm"` — neither of which any `candle-*` backend
// feature implies, so candle-only builds (e.g. `candle-trocr` alone) cannot see it. ~keep
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY as PROCESSED_HEIGHT_KEY;
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY as PROCESSED_WIDTH_KEY;

/// Default OCR language when none is configured, matching
/// `crate::core::config::ocr::DEFAULT_OCR_LANGUAGE`. Not reused directly for the
/// same feature-gating reason as the metadata keys above:
/// `OcrConfig::effective_languages` is only compiled under `feature = "ocr"` /
/// `"ocr-wasm"` / `paddle_ocr` / `liter-llm`. ~keep
const DEFAULT_LANGUAGE: &str = "eng";

/// The languages the caller requested, with blanks dropped and the documented
/// default applied when none remain. These candle backends do not perform
/// independent language detection, so this reports the requested languages
/// rather than a detected set — matching the Sceptre backend's `languages`
/// field for the same reason.
fn effective_languages(config: &OcrConfig) -> Vec<String> {
    let langs: Vec<String> = config
        .language
        .iter()
        .map(|lang| lang.trim())
        .filter(|lang| !lang.is_empty())
        .map(str::to_string)
        .collect();
    if langs.is_empty() {
        vec![DEFAULT_LANGUAGE.to_string()]
    } else {
        langs
    }
}

/// Build the full `ExtractedDocument` a candle OCR backend should return.
///
/// Populates `metadata` (`FormatMetadata::Ocr`, `ocr_used`, processed image
/// dimensions read from `image_bytes`), `tables` (parsed out of any GFM tables
/// present in `content`), and `detected_languages` (the languages the caller
/// requested; these backends do not perform independent language detection).
pub(crate) fn build_ocr_document(
    content: String,
    formulas: Vec<Formula>,
    mime_type: Cow<'static, str>,
    image_bytes: &[u8],
    config: &OcrConfig,
) -> ExtractedDocument {
    let tables = extract_gfm_tables(&content);
    let metadata = build_metadata(image_bytes, tables.len() as u32);

    ExtractedDocument {
        content,
        formulas,
        mime_type,
        metadata,
        tables,
        detected_languages: Some(effective_languages(config)),
        ..Default::default()
    }
}

/// Build OCR metadata: `FormatMetadata::Ocr` with the table count, `ocr_used`,
/// and processed image dimensions (best-effort; a decode failure just omits
/// the dimensions rather than failing the whole OCR call, since dimensions
/// are diagnostic, not load-bearing).
fn build_metadata(image_bytes: &[u8], table_count: u32) -> Metadata {
    let mut metadata = Metadata {
        format: Some(FormatMetadata::Ocr(OcrMetadata {
            table_count,
            ..Default::default()
        })),
        ocr_used: true,
        ..Default::default()
    };

    if let Some((width, height)) = probe_image_dimensions(image_bytes) {
        metadata
            .additional
            .insert(Cow::Borrowed(PROCESSED_WIDTH_KEY), serde_json::json!(width));
        metadata
            .additional
            .insert(Cow::Borrowed(PROCESSED_HEIGHT_KEY), serde_json::json!(height));
    }

    metadata
}

/// Read image dimensions from the header without decoding pixel data.
fn probe_image_dimensions(image_bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(image_bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Parse every GFM table in `content` into structured [`Table`] entries.
///
/// GLM-OCR and PaddleOCR-VL emit GFM tables directly in their markdown output;
/// without this, `tables[]` stayed empty even though the table data was
/// present in `content`. The markdown rendering of each parsed table reuses
/// [`crate::rendering::common::render_table_markdown`] (the same renderer
/// `extractors/markdown.rs` uses for tables) rather than re-serializing ad hoc,
/// so the round-tripped markdown matches the rest of the codebase's table
/// formatting.
fn extract_gfm_tables(content: &str) -> Vec<Table> {
    let mut tables = Vec::new();
    let mut in_table = false;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;

    for event in Parser::new_ext(content, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                rows.clear();
            }
            Event::End(TagEnd::Table) if in_table => {
                in_table = false;
                if !rows.is_empty() {
                    let cells = std::mem::take(&mut rows);
                    let markdown = crate::rendering::common::render_table_markdown(&cells);
                    tables.push(Table {
                        cells,
                        markdown,
                        page_number: 1,
                        ..Default::default()
                    });
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) if in_table => {
                current_row.clear();
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) if in_table && !current_row.is_empty() => {
                rows.push(std::mem::take(&mut current_row));
            }
            Event::Start(Tag::TableCell) if in_table => {
                in_cell = true;
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) if in_table => {
                in_cell = false;
                current_row.push(current_cell.trim().to_string());
                current_cell.clear();
            }
            Event::Text(text) | Event::Code(text) if in_table && in_cell => {
                current_cell.push_str(&text);
            }
            _ => {}
        }
    }

    tables
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_extract_no_tables_from_plain_text() {
        let tables = extract_gfm_tables("Just some OCR'd prose with no tables.");
        assert!(tables.is_empty(), "expected no tables; got: {tables:?}");
    }

    #[test]
    fn should_extract_single_gfm_table_from_content() {
        let content = "Some text before.\n\n\
            | Name | Age |\n\
            |------|-----|\n\
            | Alice | 30 |\n\
            | Bob | 25 |\n\n\
            Some text after.";

        let tables = extract_gfm_tables(content);

        assert_eq!(tables.len(), 1, "expected exactly one table; got: {tables:?}");
        assert_eq!(
            tables[0].cells,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
                vec!["Bob".to_string(), "25".to_string()],
            ]
        );
        assert_eq!(tables[0].page_number, 1);
        assert!(tables[0].markdown.contains("Name"));
        assert!(tables[0].markdown.contains("Alice"));
    }

    #[test]
    fn should_extract_multiple_gfm_tables_from_content() {
        let content = "| A |\n|---|\n| 1 |\n\ntext between\n\n| B |\n|---|\n| 2 |";

        let tables = extract_gfm_tables(content);

        assert_eq!(tables.len(), 2, "expected two tables; got: {tables:?}");
        assert_eq!(tables[0].cells, vec![vec!["A".to_string()], vec!["1".to_string()]]);
        assert_eq!(tables[1].cells, vec![vec!["B".to_string()], vec!["2".to_string()]]);
    }

    #[test]
    fn should_build_ocr_document_with_metadata_tables_and_languages() {
        // 1x1 PNG (smallest valid PNG payload), used only to exercise the
        // dimension probe without depending on network or real OCR output.
        let png_1x1: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00,
            0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x18,
            0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let content = "| H |\n|---|\n| v |".to_string();
        let config = OcrConfig {
            language: vec!["deu".to_string()],
            ..Default::default()
        };

        let doc = build_ocr_document(
            content.clone(),
            Vec::new(),
            Cow::Borrowed("text/markdown"),
            png_1x1,
            &config,
        );

        assert_eq!(doc.content, content);
        assert!(doc.metadata.ocr_used);
        let Some(FormatMetadata::Ocr(ocr_metadata)) = &doc.metadata.format else {
            panic!("expected FormatMetadata::Ocr; got: {:?}", doc.metadata.format);
        };
        assert_eq!(ocr_metadata.table_count, 1);
        assert_eq!(
            doc.metadata.additional.get(PROCESSED_WIDTH_KEY),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            doc.metadata.additional.get(PROCESSED_HEIGHT_KEY),
            Some(&serde_json::json!(1))
        );
        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.detected_languages, Some(vec!["deu".to_string()]));
    }
}
