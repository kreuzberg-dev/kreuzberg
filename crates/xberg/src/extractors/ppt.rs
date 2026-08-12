//! Native PPT extractor for PowerPoint 97-2003 binary format.
//!
//! Extracts text directly from OLE/CFB compound documents without LibreOffice.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::core::mime::LEGACY_POWERPOINT_MIME_TYPE;
use crate::extraction::ppt::PptSlideText;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
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
    /// Build an `InternalDocument` from PPT extracted slides, speaker notes,
    /// and embedded images.
    ///
    /// `slides` carries the deck's real per-slide structure (persist order
    /// and numbering, from `extraction::ppt::extract_texts_from_records`) --
    /// slide numbers are read from that structure, never re-derived by
    /// splitting rendered text (#1418).
    fn build_internal_document(
        slides: &[PptSlideText],
        speaker_notes: &[String],
        images: &[ExtractedImage],
    ) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("ppt");

        for (i, slide) in slides.iter().enumerate() {
            let trimmed = slide.text.trim();
            let mut lines = trimmed.lines();
            let first_line = lines.next().unwrap_or("");
            let title = if !first_line.is_empty() && first_line.len() <= 80 && lines.clone().next().is_some() {
                Some(first_line)
            } else {
                None
            };
            builder.push_slide(slide.number, title, None);

            if !trimmed.is_empty() {
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
            }

            if let Some(notes) = speaker_notes.get(i)
                && !notes.is_empty()
            {
                let key = format!("slide-{}-notes", slide.number);
                builder.push_footnote_definition(notes, &key, None);
            }
        }

        for image in images {
            builder.push_image(None, image.clone(), image.page_number, None);
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
        let extract_images = config.needs_image_data();

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
                    crate::extraction::ppt::extract_ppt_text_with_options(
                        &content_owned,
                        include_master_slides,
                        extract_images,
                    )
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("PPT extraction task failed: {e}")))?
            } else {
                crate::extraction::ppt::extract_ppt_text_with_options(content, include_master_slides, extract_images)
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                crate::extraction::ppt::extract_ppt_text_with_options(content, include_master_slides, extract_images)
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

        let mut doc = Self::build_internal_document(&result.slides, &result.speaker_notes, &result.images);
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

    /// #1418 root-cause regression at the consumer side: `build_internal_document`
    /// must trust the structured `slides` list, never re-split a slide's own
    /// text on `"\n\n"`. A single slide whose text happens to contain an
    /// internal blank line must still produce exactly one `Slide` element.
    #[test]
    fn should_produce_one_slide_element_when_slide_text_contains_internal_blank_line() {
        let slides = vec![PptSlideText {
            number: 1,
            text: "Title\n\nBody".to_string(),
        }];
        let doc = PptExtractor::build_internal_document(&slides, &[], &[]);

        let slide_numbers: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|e| match &e.kind {
                crate::types::internal::ElementKind::Slide { number } => Some(*number),
                _ => None,
            })
            .collect();

        assert_eq!(
            slide_numbers,
            vec![1],
            "one Slide entry must produce exactly one Slide element, however its text is shaped"
        );
    }

    /// #1418: a slide with no text atoms must still get a `Slide` element
    /// carrying its real persist-order number, not be dropped (which would
    /// shift every later slide's number down).
    #[test]
    fn should_number_slide_elements_by_persist_order_including_an_empty_middle_slide() {
        let slides = vec![
            PptSlideText {
                number: 1,
                text: "Slide One".to_string(),
            },
            PptSlideText {
                number: 2,
                text: String::new(),
            },
            PptSlideText {
                number: 3,
                text: "Slide Three".to_string(),
            },
        ];
        let doc = PptExtractor::build_internal_document(&slides, &[], &[]);

        let slide_numbers: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|e| match &e.kind {
                crate::types::internal::ElementKind::Slide { number } => Some(*number),
                _ => None,
            })
            .collect();

        assert_eq!(slide_numbers, vec![1, 2, 3]);
    }

    /// #1417: images recovered from the `Pictures` stream must be attached
    /// to the document (`InternalDocument::images`), not silently discarded.
    #[test]
    fn should_attach_images_to_internal_document_when_images_are_present() {
        let slides = vec![PptSlideText {
            number: 1,
            text: "Slide One".to_string(),
        }];
        let image = ExtractedImage {
            data: bytes::Bytes::from_static(b"\xFF\xD8\xFFfake-jpeg"),
            format: Cow::Borrowed("jpeg"),
            image_index: 0,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };
        let doc = PptExtractor::build_internal_document(&slides, &[], std::slice::from_ref(&image));

        assert_eq!(doc.images.len(), 1);
        assert_eq!(doc.images[0].format, "jpeg");
        assert_eq!(&doc.images[0].data[..], b"\xFF\xD8\xFFfake-jpeg");
        let has_image_element = doc
            .elements
            .iter()
            .any(|e| matches!(&e.kind, crate::types::internal::ElementKind::Image { image_index: 0 }));
        assert!(has_image_element, "an Image element must reference the pushed image");
    }

    /// #87/#1418 end-to-end: `simple.ppt` has exactly two `Slide` (0x03EE)
    /// containers (see `extraction::ppt::tests::test_extract_ppt_real_file_reports_two_slides`).
    /// The document structure produced through the real extraction pipeline
    /// must report exactly slide numbers `[1, 2]`, not ordinals derived from
    /// re-splitting joined text.
    #[tokio::test]
    async fn should_report_exact_contiguous_slide_numbers_for_real_ppt_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/ppt/simple.ppt");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test PPT");
        let extractor = PptExtractor::new();
        let config = ExtractionConfig::default();
        let doc = extractor
            .extract_content(&content, "application/vnd.ms-powerpoint", &config)
            .await
            .expect("PPT extraction failed");

        let slide_numbers: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|e| match &e.kind {
                crate::types::internal::ElementKind::Slide { number } => Some(*number),
                _ => None,
            })
            .collect();

        assert_eq!(
            slide_numbers,
            vec![1, 2],
            "simple.ppt has exactly two Slide containers, numbered 1 and 2"
        );
    }
}
