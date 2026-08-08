//! Native PPT extractor for PowerPoint 97-2003 binary format.
//!
//! Extracts text directly from OLE/CFB compound documents without LibreOffice.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::core::mime::LEGACY_POWERPOINT_MIME_TYPE;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::{Metadata, PageInfo, PageStructure, PageUnitType};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
#[cfg_attr(alef, alef(skip))]
/// Native PPT extractor using OLE/CFB parsing.
///
/// This extractor handles PowerPoint 97-2003 binary (.ppt) files without
/// requiring LibreOffice, providing ~50x faster extraction.
pub struct PptExtractor;

impl PptExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for PptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PptExtractor {
    /// Build an `InternalDocument` from PPT extracted text and speaker notes.
    ///
    /// Splits text by double-newlines; each block corresponds to a slide.
    fn build_internal_document(text: &str, speaker_notes: &[String]) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("ppt");

        let slide_blocks: Vec<&str> = text.split("\n\n").collect();
        for (i, block) in slide_blocks.iter().enumerate() {
            let trimmed = block.trim();
            if !trimmed.is_empty() {
                let slide_num = (i + 1) as u32;
                let mut lines = trimmed.lines();
                let first_line = lines.next().unwrap_or("");
                let title = if first_line.len() <= 80 && lines.clone().next().is_some() {
                    Some(first_line)
                } else {
                    None
                };
                builder.push_slide(slide_num, title, None);

                if title.is_some() {
                    for line in lines {
                        let lt = line.trim();
                        if !lt.is_empty() {
                            builder.push_paragraph(lt, vec![], None, None);
                        }
                    }
                } else {
                    builder.push_paragraph(trimmed, vec![], None, None);
                }

                if let Some(notes) = speaker_notes.get(i)
                    && !notes.is_empty()
                {
                    let key = format!("slide-{}-notes", slide_num);
                    builder.push_footnote_definition(notes, &key, None);
                }
            }
        }

        builder.build()
    }
}

impl Plugin for PptExtractor {
    fn name(&self) -> &str {
        "ppt-extractor"
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
        "Native PPT text extraction via OLE/CFB parsing"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PptExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let include_master_slides = config.content_filter.as_ref().is_some_and(|f| f.include_headers);

        let result = {
            #[cfg(feature = "tokio-runtime")]
            if crate::core::batch_mode::is_batch_mode() {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let content_owned = content.to_vec();
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || -> crate::error::Result<_> {
                    let _guard = span.entered();
                    crate::extraction::ppt::extract_ppt_text_with_options(&content_owned, include_master_slides)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("PPT extraction task failed: {e}")))?
            } else {
                crate::extraction::ppt::extract_ppt_text_with_options(content, include_master_slides)
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                crate::extraction::ppt::extract_ppt_text_with_options(content, include_master_slides)
            }
        }?;

        let mut metadata_map = AHashMap::new();

        let meta_title = result.metadata.title;
        let meta_subject = result.metadata.subject;

        let (meta_authors, meta_created_by) = if let Some(author) = result.metadata.author {
            (Some(vec![author.clone()]), Some(author))
        } else {
            (None, None)
        };

        let meta_modified_by = result.metadata.last_author;

        metadata_map.insert(
            Cow::Borrowed("slide_count"),
            serde_json::Value::Number(result.slide_count.into()),
        );
        metadata_map.insert(
            Cow::Borrowed("extraction_method"),
            serde_json::Value::String("native_ole".to_string()),
        );

        if !result.speaker_notes.is_empty() {
            metadata_map.insert(
                Cow::Borrowed("speaker_notes"),
                serde_json::Value::Array(
                    result
                        .speaker_notes
                        .iter()
                        .map(|n| serde_json::Value::String(n.clone()))
                        .collect(),
                ),
            );
        }

        let page_structure = if result.slide_count > 0 {
            Some(PageStructure {
                total_count: result.slide_count as u32,
                unit_type: PageUnitType::Slide,
                boundaries: None,
                pages: Some(
                    (1..=result.slide_count)
                        .map(|num| PageInfo {
                            number: num as u32,
                            title: None,
                            dimensions: None,
                            image_count: None,
                            table_count: None,
                            hidden: None,
                            is_blank: None,
                            has_vector_graphics: false,
                        })
                        .collect(),
                ),
            })
        } else {
            None
        };

        let mut doc = Self::build_internal_document(&result.text, &result.speaker_notes);
        doc.mime_type = mime_type.to_string();
        doc.processing_warnings.extend(result.processing_warnings);
        doc.metadata = Metadata {
            title: meta_title,
            subject: meta_subject,
            authors: meta_authors,
            created_by: meta_created_by,
            modified_by: meta_modified_by,
            pages: page_structure,
            additional: metadata_map,
            ..Default::default()
        };

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[LEGACY_POWERPOINT_MIME_TYPE]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ppt_extractor_plugin_interface() {
        let extractor = PptExtractor::new();
        assert_eq!(extractor.name(), "ppt-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert_eq!(extractor.supported_mime_types(), &["application/vnd.ms-powerpoint"]);
    }

    #[tokio::test]
    async fn test_ppt_extractor_initialize_shutdown() {
        let extractor = PptExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[tokio::test]
    async fn test_ppt_extractor_real_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let extractor = PptExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(&content, "application/vnd.ms-powerpoint", &config)
            .await
            .expect("PPT extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(!result.content.is_empty(), "Should extract text from PPT");
        assert_eq!(&*result.mime_type, "application/vnd.ms-powerpoint");
    }

    #[tokio::test]
    async fn test_ppt_document_structure_slides() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let extractor = PptExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(&content, "application/vnd.ms-powerpoint", &config)
            .await
            .expect("PPT extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(result.document.is_some(), "Should produce document structure for PPT");
        let doc = result.document.unwrap();
        let has_slide = doc
            .nodes
            .iter()
            .any(|n| matches!(n.content, crate::types::document_structure::NodeContent::Slide { .. }));
        assert!(has_slide, "PPT should produce Slide nodes in document structure");
    }

    #[tokio::test]
    async fn test_ppt_slide_count_metadata() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let extractor = PptExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(&content, "application/vnd.ms-powerpoint", &config)
            .await
            .expect("PPT extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(
            result.metadata.additional.contains_key("slide_count"),
            "Should have slide_count metadata"
        );
        let slide_count = result.metadata.additional.get("slide_count").unwrap();
        assert!(slide_count.as_u64().unwrap_or(0) > 0, "Slide count should be > 0");
    }

    /// PPT speaker notes go to `metadata.additional["speaker_notes"]` as a JSON array,
    /// NOT to `PageContent.speaker_notes`.  The legacy binary format does not support
    /// per-slide `PageContent` objects, so `page_contents` is always `None` for PPT
    /// regardless of whether `page_config` is set.
    #[tokio::test]
    async fn test_ppt_speaker_notes_in_metadata_not_page_contents() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let extractor = PptExtractor::new();
        let config = ExtractionConfig {
            pages: Some(crate::core::config::PageConfig::default()),
            ..Default::default()
        };
        let result = extractor
            .extract_content(&content, "application/vnd.ms-powerpoint", &config)
            .await
            .expect("PPT extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(
            result.pages.is_none(),
            "PPT should not produce pages; speaker notes are in metadata.additional"
        );
        if let Some(notes) = result.metadata.additional.get("speaker_notes") {
            assert!(notes.is_array(), "PPT speaker_notes in metadata should be a JSON array");
        }
    }
}
