//! Vendored HWP text extraction from hwpers v0.5.0 (MIT OR Apache-2.0)
//!
//! Supports HWP 5.0 Compound File Binary (CFB) documents.  Only text
//! extraction is implemented; write, render, crypto, HWPX, and preview paths
//! from the original crate are omitted.
//!
//! # Entry point
//!
//! ```ignore
//! let text = extract_hwp_text(bytes)?;
//! ```

/// HWP equation-editor (EQEdit) script to LaTeX conversion.
mod equation;
/// Error types for HWP parsing failures.
pub mod error;
/// Document model types for extracted HWP content.
pub mod model;
/// HWP record, file-header, and body-text parsers.
pub mod parser;
/// CFB compound-file reader and decompression utilities.
pub mod reader;
/// OLE `SummaryInformation`-compatible property-set stream parsing.
mod summary;

use crate::extraction::hwp::model::HwpDocument;
use error::{HwpError, Result};
use parser::{FileHeader, parse_body_text, parse_doc_info};
use reader::CfbReader;

/// Stream name for HWP document metadata: same binary layout as the standard OLE
/// `\x05SummaryInformation` stream (MS-OLEPS), just named for HWP (#105).
const SUMMARY_INFO_STREAM: &str = "\u{5}HwpSummaryInformation";

/// Extract the structured document model from an HWP 5.0 document.
pub(crate) fn extract_hwp_document(bytes: &[u8]) -> Result<HwpDocument> {
    let mut cfb = CfbReader::from_bytes(bytes)?;

    let header_data = cfb.read_stream("FileHeader")?;
    let header = FileHeader::parse(header_data)?;

    if header.is_encrypted() {
        return Err(HwpError::UnsupportedVersion(
            "Password-encrypted HWP documents are not supported".to_string(),
        ));
    }

    let mut doc = HwpDocument::default();

    if cfb.stream_exists("DocInfo") {
        let doc_info_data = cfb.read_stream("DocInfo")?;
        if let Ok(char_shapes) = parse_doc_info(doc_info_data) {
            doc.char_shapes = char_shapes;
        }
    }

    if cfb.stream_exists(SUMMARY_INFO_STREAM) {
        match cfb.read_stream(SUMMARY_INFO_STREAM) {
            Ok(summary_data) => doc.summary_info = summary::parse_summary_information(&summary_data),
            Err(e) => doc
                .warnings
                .push(format!("HWP summary-information stream could not be read: {e}")),
        }
    }

    let mut streams = cfb.list_streams();
    streams.sort();

    for path in streams {
        if path.starts_with("BodyText/Section") {
            let section_data = cfb.read_stream(&path)?;
            match parse_body_text(section_data, header.is_compressed(), &path, &mut doc.warnings) {
                Ok(sections) => doc.sections.extend(sections),
                Err(e) => doc.warnings.push(format!(
                    "HWP section stream '{path}' could not be parsed and was skipped: {e}"
                )),
            }
        }
    }

    for path in cfb.list_streams() {
        if path.starts_with("BinData/") {
            let image_data = cfb.read_stream(&path)?;
            doc.images.push(model::HwpImage {
                name: path.clone(),
                data: image_data,
            });
        }
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_invalid_hwp() {
        let bytes = b"Not a valid HWP file";
        assert!(extract_hwp_document(bytes).is_err());
    }

    #[test]
    fn test_extract_converted_output_populates_summary_info() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/hwp/converted_output.hwp");
        if !path.exists() {
            println!("Skipping: test document not found at {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path).expect("read file");
        let doc = extract_hwp_document(&bytes).expect("HWP extraction should succeed");

        let summary = doc
            .summary_info
            .expect("converted_output.hwp has a SummaryInformation stream");
        assert_eq!(summary.title.as_deref(), Some("강사위촉계약서(예시)"));
        assert_eq!(summary.last_author.as_deref(), Some("jinsol"));
    }

    #[test]
    fn test_extract_styled_document_recovers_real_paragraph_text() {
        // Regression test for two independent silent-loss bugs, both of which made this
        // genuine HWP 5.0 document (not synthetically constructed) extract as empty:
        //
        //  1. `list_streams` returned `cfb`'s absolute paths (`/BodyText/Section0`) while
        //     the caller tested `starts_with("BodyText/Section")`, so no section — and no
        //     BinData image — was ever found.
        //  2. `TAG_PARA_HEADER`/`TAG_PARA_TEXT` held the wrong tag IDs, so once sections
        //     were found every body-text record still failed to match (#236).
        //
        // Neither produced a warning. The expected strings below are cross-checked against
        // the file's own `PrvText` preview stream.
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/hwp/styled_document.hwp");
        if !path.exists() {
            println!("Skipping: test document not found at {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path).expect("read file");
        let doc = extract_hwp_document(&bytes).expect("HWP extraction should succeed");

        let text: String = doc
            .sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .filter_map(|paragraph| paragraph.text.as_ref().map(|t| t.content.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        for expected in [
            "스타일 문서 예제",
            "이것은 굵은 글씨입니다.",
            "바탕체로 작성된 텍스트입니다.",
        ] {
            assert!(
                text.contains(expected),
                "expected {expected:?} from the document body; got {text:?}"
            );
        }
    }
}
