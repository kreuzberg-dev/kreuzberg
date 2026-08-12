//! Hangul Word Processor (.hwp) extractor.
//!
//! Extracts text content from HWP 5.0 documents using the vendored HWP parser
//! in `crate::extraction::hwp`.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::core::diagnostics::push_warning;
use crate::extraction::hwp::model::{CharShape, HwpDocument, SummaryInfo};
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use async_trait::async_trait;
use bytes::Bytes;
use std::borrow::Cow;

/// `ProcessingWarning::source` for every warning this extractor emits (#236).
const HWP_WARNING_SOURCE: &str = "hwp";
#[cfg_attr(alef, alef(skip))]
/// Extractor for Hangul Word Processor (.hwp) files.
///
/// Supports HWP 5.0 format, the standard document format in South Korea.
pub struct HwpExtractor;

impl HwpExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for HwpExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for HwpExtractor {
    fn name(&self) -> &str {
        "hwp-extractor"
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
        "Hangul Word Processor (.hwp) text extraction"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

fn extract_hwp_content(content: &[u8]) -> Result<HwpDocument> {
    crate::extraction::hwp::extract_hwp_document(content)
        .map_err(|e| crate::XbergError::parsing(format!("Failed to read HWP file: {e}")))
}

/// The LaTeX of a paragraph that holds one equation and nothing else.
///
/// The parser writes an equation record as `$latex$`, so a paragraph whose whole
/// text is a single `$`-delimited span is that equation on its own.
fn standalone_equation(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let inner = trimmed.strip_prefix('$')?.strip_suffix('$')?;
    if inner.is_empty() || inner.contains('$') {
        return None;
    }
    Some(inner)
}

/// Build an `InternalDocument` from HWP structured model.
fn build_hwp_internal_document(hwp_doc: &HwpDocument) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("hwp");

    if let Some(metadata) = build_metadata(hwp_doc.summary_info.as_ref()) {
        builder.set_metadata(metadata);
    }

    for section in &hwp_doc.sections {
        for para in &section.paragraphs {
            if let Some(ref t) = para.text
                && !t.content.is_empty()
            {
                // The parser converts an equation record to LaTeX and splices it
                // into this text between `$` delimiters. A paragraph that holds
                // nothing else is an equation object, so it becomes a formula.
                // Math mixed with prose stays in the sentence: `char_shape_runs`
                // are byte offsets into this string, so lifting a span out would
                // move every annotation after it.
                if let Some(latex) = standalone_equation(&t.content) {
                    builder.push_formula(latex, None, None);
                    continue;
                }
                let annotations = apply_char_shapes(&t.content, &para.char_shape_runs, &hwp_doc.char_shapes);
                if para.outline_level > 0 {
                    let idx = builder.push_heading(para.outline_level, &t.content, None, None);
                    if !annotations.is_empty() {
                        builder.set_annotations(idx, annotations);
                    }
                } else {
                    builder.push_paragraph(&t.content, annotations, None, None);
                }
            }
        }

        // #105/#236 — tables were previously not read from the parsed model at all.
        for table in &section.tables {
            if !table.rows.is_empty() {
                builder.push_table_from_cells(&table.rows, None, None);
            }
        }
    }

    for (idx, image) in hwp_doc.images.iter().enumerate() {
        let format = match infer::get(&image.data) {
            Some(info) => Cow::Owned(info.mime_type().to_string()),
            None => Cow::Borrowed("application/octet-stream"),
        };

        let extracted = ExtractedImage {
            data: Bytes::from(image.data.clone()),
            format,
            image_index: idx as u32,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: None,
            source_path: Some(image.name.clone()),
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };
        builder.push_image(None, extracted, None, None);
    }

    builder.build()
}

/// Maps the parsed `SummaryInformation` stream to the common `Metadata` DTO (#105).
fn build_metadata(summary: Option<&SummaryInfo>) -> Option<Metadata> {
    let summary = summary?;
    let mut metadata = Metadata {
        title: summary.title.clone(),
        subject: summary.subject.clone(),
        authors: summary.author.as_ref().map(|author| vec![author.clone()]),
        // HWP stores keywords as a single free-form string rather than a delimited
        // list, unlike most other formats' `Metadata::keywords: Vec<String>` — kept as
        // one entry rather than guessing a delimiter and splitting it incorrectly. ~keep
        keywords: summary.keywords.as_ref().map(|keywords| vec![keywords.clone()]),
        created_at: summary.created.clone(),
        modified_at: summary.modified.clone(),
        ..Default::default()
    };

    if let Some(last_author) = &summary.last_author {
        metadata.additional.insert(
            Cow::Borrowed("last_author"),
            serde_json::Value::String(last_author.clone()),
        );
    }

    if metadata.is_empty() { None } else { Some(metadata) }
}

fn apply_char_shapes(text: &str, runs: &[(u32, u16)], char_shapes: &[CharShape]) -> Vec<TextAnnotation> {
    let mut annotations = Vec::new();
    if runs.is_empty() || char_shapes.is_empty() {
        return annotations;
    }

    let char_indices: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let total_chars = char_indices.len();
    let total_bytes = text.len();

    let mut sorted_runs = runs.to_vec();
    sorted_runs.sort_by_key(|r| r.0);

    for i in 0..sorted_runs.len() {
        let (start_pos, shape_idx) = sorted_runs[i];
        let end_pos = if i + 1 < sorted_runs.len() {
            sorted_runs[i + 1].0
        } else {
            total_chars as u32
        };

        if let Some(shape) = char_shapes.get(shape_idx as usize) {
            let start_byte = char_indices.get(start_pos as usize).cloned().unwrap_or(total_bytes);
            let end_byte = char_indices.get(end_pos as usize).cloned().unwrap_or(total_bytes);

            if start_byte < end_byte {
                if shape.bold {
                    annotations.push(TextAnnotation {
                        start: start_byte as u32,
                        end: end_byte as u32,
                        kind: AnnotationKind::Bold,
                    });
                }
                if shape.italic {
                    annotations.push(TextAnnotation {
                        start: start_byte as u32,
                        end: end_byte as u32,
                        kind: AnnotationKind::Italic,
                    });
                }
                if shape.underline {
                    annotations.push(TextAnnotation {
                        start: start_byte as u32,
                        end: end_byte as u32,
                        kind: AnnotationKind::Underline,
                    });
                }
            }
        }
    }
    annotations
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for HwpExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        _config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let hwp_doc = extract_hwp_content(content)?;
        if hwp_doc.sections.is_empty() {
            return Err(crate::XbergError::parsing(
                "no BodyText sections found in HWP document".to_string(),
            ));
        }
        let mut doc = build_hwp_internal_document(&hwp_doc);
        if doc.elements.is_empty() {
            return Err(crate::XbergError::parsing(
                "no BodyText sections found in HWP document".to_string(),
            ));
        }
        doc.mime_type = mime_type.to_string();
        // #236 — name what parsing could not recover instead of returning `Ok` with
        // no indication that some body-text content was abandoned.
        for warning in &hwp_doc.warnings {
            push_warning(&mut doc.processing_warnings, HWP_WARNING_SOURCE, warning.clone());
        }
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-hwp"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::hwp::model::{HwpTable, Section};

    #[test]
    fn test_build_metadata_returns_none_for_absent_summary_info() {
        assert!(build_metadata(None).is_none());
    }

    #[test]
    fn test_build_metadata_returns_none_for_all_empty_summary_info() {
        let summary = SummaryInfo::default();
        assert!(build_metadata(Some(&summary)).is_none());
    }

    #[test]
    fn test_build_metadata_maps_summary_info_fields() {
        let summary = SummaryInfo {
            title: Some("계약서".to_string()),
            subject: Some("Subject".to_string()),
            author: Some("Author".to_string()),
            keywords: Some("k1, k2".to_string()),
            comments: None,
            last_author: Some("jinsol".to_string()),
            created: Some("2024-07-01T06:05:58Z".to_string()),
            modified: Some("2024-07-02T04:05:59Z".to_string()),
        };

        let metadata = build_metadata(Some(&summary)).expect("must produce metadata");
        assert_eq!(metadata.title.as_deref(), Some("계약서"));
        assert_eq!(metadata.subject.as_deref(), Some("Subject"));
        assert_eq!(metadata.authors, Some(vec!["Author".to_string()]));
        assert_eq!(metadata.keywords, Some(vec!["k1, k2".to_string()]));
        assert_eq!(metadata.created_at.as_deref(), Some("2024-07-01T06:05:58Z"));
        assert_eq!(metadata.modified_at.as_deref(), Some("2024-07-02T04:05:59Z"));
        assert_eq!(
            metadata.additional.get("last_author"),
            Some(&serde_json::Value::String("jinsol".to_string()))
        );
    }

    #[test]
    fn test_build_hwp_internal_document_includes_tables() {
        let section = Section {
            paragraphs: vec![],
            tables: vec![HwpTable {
                rows: vec![
                    vec!["Name".to_string(), "Age".to_string()],
                    vec!["Alice".to_string(), "30".to_string()],
                ],
            }],
        };
        let hwp_doc = HwpDocument {
            sections: vec![section],
            ..HwpDocument::default()
        };

        let internal_doc = build_hwp_internal_document(&hwp_doc);

        assert_eq!(internal_doc.tables.len(), 1);
        assert_eq!(
            internal_doc.tables[0].cells,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ]
        );
    }

    #[tokio::test]
    async fn test_extract_content_real_styled_document_recovers_text_and_metadata() {
        // End-to-end regression test through the public `InternalDocumentExtractor`
        // API (not just `extract_hwp_document`) for the #236 tag-ID fix: before it,
        // this genuine HWP 5.0 document produced zero elements and therefore always
        // failed with "no BodyText sections found".
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/hwp/styled_document.hwp");
        if !path.exists() {
            println!("Skipping: test document not found at {}", path.display());
            return;
        }
        let content = std::fs::read(&path).expect("read file");
        let extractor = HwpExtractor::new();
        let result = extractor
            .extract_content(&content, "application/x-hwp", &ExtractionConfig::default())
            .await
            .expect("extraction of styled_document.hwp must succeed");

        let text: String = result.elements.iter().map(|element| element.text.as_str()).collect();
        assert!(!text.trim().is_empty(), "expected non-empty extracted text");
    }

    #[test]
    fn test_hwp_extractor_plugin_interface() {
        let extractor = HwpExtractor::new();
        assert_eq!(extractor.name(), "hwp-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert_eq!(extractor.supported_mime_types(), &["application/x-hwp"]);
    }

    #[test]
    fn test_hwp_extractor_initialize_shutdown() {
        let extractor = HwpExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_apply_char_shapes() {
        let text = "Hello world";
        let runs = vec![(0, 0), (5, 1)];
        let shape1 = CharShape {
            bold: true,
            ..Default::default()
        };
        let shape2 = CharShape {
            italic: true,
            ..Default::default()
        };
        let char_shapes = vec![shape1, shape2];

        let annotations = apply_char_shapes(text, &runs, &char_shapes);
        assert_eq!(annotations.len(), 2);
        assert_eq!(annotations[0].kind, AnnotationKind::Bold);
        assert_eq!(annotations[0].start, 0);
        assert_eq!(annotations[0].end, 5);

        assert_eq!(annotations[1].kind, AnnotationKind::Italic);
        assert_eq!(annotations[1].start, 5);
        assert_eq!(annotations[1].end, 11);
    }

    #[test]
    fn test_build_hwp_internal_document() {
        use crate::extraction::hwp::model::{HwpImage, ParaText, Paragraph, Section};

        let shape1 = CharShape {
            bold: true,
            ..Default::default()
        };

        let para = Paragraph {
            outline_level: 1,
            text: Some(ParaText {
                content: "My Heading".to_string(),
            }),
            char_shape_runs: vec![(0, 0)],
        };

        let section = Section {
            paragraphs: vec![para],
            tables: vec![],
        };

        let image = HwpImage {
            name: "image1.png".to_string(),
            data: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        };

        let hwp_doc = HwpDocument {
            char_shapes: vec![shape1],
            sections: vec![section],
            images: vec![image],
            ..HwpDocument::default()
        };

        let internal_doc = build_hwp_internal_document(&hwp_doc);

        assert_eq!(internal_doc.elements.len(), 2);

        let first_elem = &internal_doc.elements[0];
        match first_elem.kind {
            crate::types::internal::ElementKind::Heading { level } => {
                assert_eq!(level, 1);
                assert_eq!(first_elem.text, "My Heading");
                assert_eq!(first_elem.annotations.len(), 1);
                assert_eq!(first_elem.annotations[0].kind, AnnotationKind::Bold);
            }
            _ => panic!("Expected Heading"),
        }

        let second_elem = &internal_doc.elements[1];
        match second_elem.kind {
            crate::types::internal::ElementKind::Image { image_index } => {
                let i = &internal_doc.images[image_index as usize];
                assert_eq!(i.source_path.as_deref(), Some("image1.png"));
                assert_eq!(i.format, Cow::Borrowed("image/png"));
            }
            _ => panic!("Expected Image"),
        }
    }

    #[test]
    fn test_hwpx_mime_not_routed_to_hwp_extractor() {
        use crate::XbergError;
        use crate::plugins::registry::DocumentExtractorRegistry;
        use std::sync::Arc;

        let mut registry = DocumentExtractorRegistry::new();
        registry.register(Arc::new(HwpExtractor::new())).unwrap();

        let result = registry.get("application/haansofthwpx");
        assert!(
            matches!(result, Err(XbergError::UnsupportedFormat(_))),
            "application/haansofthwpx must not be routed to HwpExtractor"
        );
    }
    #[test]
    fn test_standalone_equation_is_recognised() {
        assert_eq!(standalone_equation("$\\frac{a}{b}$"), Some("\\frac{a}{b}"));
        assert_eq!(standalone_equation("  $x^2$  "), Some("x^2"));
    }

    /// Math mixed with prose stays in the sentence, and a paragraph holding two
    /// spans is prose about math rather than one equation.
    #[test]
    fn test_mixed_paragraph_is_not_a_standalone_equation() {
        assert_eq!(standalone_equation("Result: $x^2$"), None);
        assert_eq!(standalone_equation("$a$ and $b$"), None);
        assert_eq!(standalone_equation("plain text"), None);
        assert_eq!(standalone_equation("$$"), None);
    }

}
