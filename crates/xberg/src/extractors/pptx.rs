//! PowerPoint presentation extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use crate::types::uri::ExtractedUri;
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
use std::path::Path;
#[cfg_attr(alef, alef(skip))]
/// PowerPoint presentation extractor.
///
/// Supports: .pptx, .pptm, .ppsx
pub struct PptxExtractor;

impl Default for PptxExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PptxExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl PptxExtractor {
    /// Build an `InternalDocument` from PPTX extracted text.
    ///
    /// Splits content by double-newlines and uses internal slide markers to
    /// retain the archive-derived slide number while rebuilding elements.
    ///
    /// Note: For richer structure, the builder should be integrated into
    /// `crate::extraction::pptx` alongside the existing `DocumentStructure` building.
    /// Try to strip an ordered-list prefix like `1. `, `2. `, `10. ` from a line.
    /// Returns the remaining text after the prefix, or `None` if the line does not
    /// start with a `<digits>. ` pattern.
    fn strip_ordered_prefix(line: &str) -> Option<&str> {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i + 2 > bytes.len() {
            return None;
        }
        if bytes[i] == b'.' && bytes[i + 1] == b' ' {
            Some(&line[i + 2..])
        } else {
            None
        }
    }

    fn build_internal_document(
        content: &str,
        slide_count: u32,
        budget: &mut SecurityBudget,
    ) -> Result<InternalDocument> {
        let mut builder = InternalDocumentBuilder::new("pptx");
        let mut slide_num: u32 = 0;
        let mut in_notes = false;
        let has_slide_markers = content.contains("<!-- Slide number: ");

        let blocks: Vec<&str> = content.split("\n\n").collect();

        for block in &blocks {
            budget.step()?;
            let trimmed = block.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(marker) = trimmed
                .strip_prefix("<!-- Slide number: ")
                .and_then(|value| value.strip_suffix(" -->"))
                && let Ok(number) = marker.parse::<u32>()
                && (1..=slide_count).contains(&number)
            {
                slide_num = number;
                in_notes = false;
                continue;
            }

            if trimmed.starts_with("### Notes:") || trimmed == "Notes:" {
                in_notes = true;
                continue;
            }

            if let Some(title_text) = trimmed.strip_prefix("# ") {
                in_notes = false;
                if !has_slide_markers {
                    slide_num += 1;
                }
                let title = title_text.trim();
                if !title.is_empty() {
                    budget.account_text(title.len())?;
                    builder.push_heading(2, title, None, None);
                }
                continue;
            }

            if in_notes {
                in_notes = false;
            }

            if trimmed.starts_with('|') {
                let cells = Self::parse_markdown_table(trimmed);
                if !cells.is_empty() {
                    builder.push_table_from_cells(&cells, Some(slide_num), None);
                }
                continue;
            }

            let mut in_list: Option<bool> = None;

            for line in trimmed.lines() {
                let lt = line.trim();
                if lt.is_empty() {
                    if in_list.is_some() {
                        builder.end_list();
                        in_list = None;
                    }
                    continue;
                }

                let list_match = if let Some(item_text) = lt.strip_prefix("- ") {
                    Some((false, item_text))
                } else {
                    Self::strip_ordered_prefix(lt).map(|item_text| (true, item_text))
                };

                if let Some((ordered, item_text)) = list_match {
                    match in_list {
                        Some(prev_ordered) if prev_ordered != ordered => {
                            builder.end_list();
                            builder.push_list(ordered);
                            in_list = Some(ordered);
                        }
                        None => {
                            builder.push_list(ordered);
                            in_list = Some(ordered);
                        }
                        _ => {}
                    }
                    budget.account_text(item_text.len())?;
                    builder.push_list_item(item_text, ordered, vec![], Some(slide_num), None);
                } else {
                    if in_list.is_some() {
                        builder.end_list();
                        in_list = None;
                    }
                    budget.account_text(lt.len())?;
                    builder.push_paragraph(lt, vec![], None, None);
                }
            }

            if in_list.is_some() {
                builder.end_list();
            }
        }

        if slide_num == 0 && slide_count > 0 {
            builder.push_slide(1, None, Some(1));
        }

        Ok(builder.build())
    }

    /// Parse a markdown table block into a 2D cell grid.
    fn parse_markdown_table(table_text: &str) -> Vec<Vec<String>> {
        let mut cells = Vec::new();
        for line in table_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.contains("---") {
                continue;
            }
            let row: Vec<String> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect();
            if !row.is_empty() {
                cells.push(row);
            }
        }
        cells
    }
}

impl PptxExtractor {
    /// Build an InternalDocument from a PptxExtractionResult, mapping office
    /// metadata to standard `Metadata` struct fields.
    ///
    /// `budget` is threaded into the internal document builder to enforce
    /// hostile-input limits on the extracted content.
    fn build_document_from_result(
        pptx_result: crate::types::PptxExtractionResult,
        mime_type: &str,
        extract_images: bool,
        budget: &mut SecurityBudget,
    ) -> Result<InternalDocument> {
        let mut additional: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();

        let mut pptx_metadata = pptx_result.metadata;
        pptx_metadata.image_count = Some(pptx_result.image_count as u32);
        pptx_metadata.table_count = Some(pptx_result.table_count as u32);

        let office_meta = &pptx_result.office_metadata;
        let title = office_meta.get("title").cloned();
        let subject = office_meta.get("subject").cloned();
        let created_by = office_meta.get("created_by").cloned();
        let modified_by = office_meta.get("modified_by").cloned();
        let created_at = office_meta.get("created_at").cloned();
        let modified_at = office_meta.get("modified_at").cloned();
        let authors = office_meta.get("author").map(|a| vec![a.clone()]);
        let keywords = office_meta.get("keywords").map(|k| {
            k.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });

        for (key, value) in &pptx_result.office_metadata {
            match key.as_str() {
                "title" | "subject" | "created_by" | "modified_by" | "created_at" | "modified_at" | "author"
                | "keywords" => {}
                "slide_count" => {}
                "notes_count" | "hidden_slides" => {
                    let json_value = value
                        .parse::<u64>()
                        .map(|n| serde_json::Value::Number(n.into()))
                        .unwrap_or_else(|_| serde_json::json!(value));
                    additional.insert(Cow::Owned(key.clone()), json_value);
                }
                _ => {
                    additional.insert(Cow::Owned(key.clone()), serde_json::json!(value));
                }
            }
        }

        let mut doc = Self::build_internal_document(&pptx_result.content, pptx_result.slide_count as u32, budget)?;
        doc.mime_type = mime_type.to_string();

        let mut metadata = Metadata {
            title,
            subject,
            authors,
            keywords,
            created_at,
            modified_at,
            created_by,
            modified_by,
            format: Some(crate::types::FormatMetadata::Pptx(pptx_metadata)),
            additional,
            ..Default::default()
        };

        if let Some(page_structure) = pptx_result.page_structure {
            metadata.pages = Some(page_structure);
        }

        doc.metadata = metadata;

        for (url, label) in pptx_result.hyperlinks {
            doc.push_uri(ExtractedUri::hyperlink(&url, label));
        }

        doc.prebuilt_pages = pptx_result.page_contents;

        doc.revisions = pptx_result.revisions;

        if extract_images {
            doc.images = pptx_result.images;
        }

        Ok(doc)
    }
}

impl Plugin for PptxExtractor {
    fn name(&self) -> &str {
        "pptx-extractor"
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

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PptxExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "pptx", size_bytes = content.len(), "extraction starting");
        let extract_images = config.needs_image_data();
        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);
        let plain = matches!(config.output_format, crate::core::config::OutputFormat::Plain);

        let mut pptx_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

        let pptx_result = {
            #[cfg(feature = "tokio-runtime")]
            {
                if crate::core::batch_mode::is_batch_mode() {
                    if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                        return Err(crate::error::XbergError::Cancelled);
                    }
                    let content_owned = content.to_vec();
                    let options = crate::extraction::pptx::PptxExtractionOptions {
                        extract_images,
                        page_config: config.pages.clone(),
                        plain,
                        include_structure: false,
                        inject_placeholders,
                    };
                    let span = tracing::Span::current();
                    let (result, warnings) = tokio::task::spawn_blocking(move || {
                        let _guard = span.entered();
                        let mut warnings = Vec::new();
                        let result =
                            crate::extraction::pptx::extract_pptx_from_bytes(&content_owned, &options, &mut warnings);
                        (result, warnings)
                    })
                    .await
                    .map_err(|e| crate::error::XbergError::parsing(format!("PPTX extraction task failed: {}", e)))?;
                    pptx_warnings = warnings;
                    result?
                } else {
                    let options = crate::extraction::pptx::PptxExtractionOptions {
                        extract_images,
                        page_config: config.pages.clone(),
                        plain,
                        include_structure: false,
                        inject_placeholders,
                    };
                    crate::extraction::pptx::extract_pptx_from_bytes(content, &options, &mut pptx_warnings)?
                }
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                let options = crate::extraction::pptx::PptxExtractionOptions {
                    extract_images,
                    page_config: config.pages.clone(),
                    plain,
                    include_structure: false,
                    inject_placeholders,
                };
                crate::extraction::pptx::extract_pptx_from_bytes(content, &options, &mut pptx_warnings)?
            }
        };

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = Self::build_document_from_result(pptx_result, mime_type, extract_images, &mut budget)?;
        doc.processing_warnings.extend(pptx_warnings);

        if config.max_archive_depth > 0 {
            let (children, embed_warnings) = crate::extraction::ooxml_embedded::extract_ooxml_embedded_objects(
                content,
                "ppt/embeddings/",
                "pptx",
                config,
            )
            .await;
            if !children.is_empty() {
                doc.children = Some(children);
            }
            doc.processing_warnings.extend(embed_warnings);
        }

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "pptx",
            "extraction complete"
        );
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

        let extract_images = config.needs_image_data();
        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);
        let plain = matches!(config.output_format, crate::core::config::OutputFormat::Plain);

        let options = crate::extraction::pptx::PptxExtractionOptions {
            extract_images,
            page_config: config.pages.clone(),
            plain,
            include_structure: false,
            inject_placeholders,
        };
        let mut pptx_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();
        let pptx_result = crate::extraction::pptx::extract_pptx_from_path(path_str, &options, &mut pptx_warnings)?;

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = Self::build_document_from_result(pptx_result, mime_type, extract_images, &mut budget)?;
        doc.processing_warnings.extend(pptx_warnings);
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.ms-powerpoint.presentation.macroEnabled.12",
            "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
            "application/vnd.openxmlformats-officedocument.presentationml.template",
            "application/vnd.ms-powerpoint.template.macroEnabled.12",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pptx_extractor_plugin_interface() {
        let extractor = PptxExtractor::new();
        assert_eq!(extractor.name(), "pptx-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_pptx_extractor_supported_mime_types() {
        let extractor = PptxExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 5);
        assert!(mime_types.contains(&"application/vnd.openxmlformats-officedocument.presentationml.presentation"));
    }

    #[test]
    fn test_internal_slide_markers_set_table_and_list_page_numbers() {
        use crate::types::internal::ElementKind;

        let content = concat!(
            "<!-- Slide number: 1 -->\n\n",
            "# Titled slide\n\n",
            "Introduction\n\n",
            "<!-- Slide number: 2 -->\n\n",
            "This untitled slide has a paragraph long enough not to be inferred as a title. ",
            "It deliberately contains more than one hundred characters in total.\n\n",
            "<!-- Slide number: 3 -->\n\n",
            "| Name | Value |\n",
            "| --- | --- |\n",
            "| answer | 42 |\n\n",
            "- final item",
        );
        let mut budget = SecurityBudget::with_defaults();

        let document = PptxExtractor::build_internal_document(content, 3, &mut budget)
            .expect("internal PPTX document should build");

        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].page_number, 3);

        let list_item = document
            .elements
            .iter()
            .find(|element| matches!(element.kind, ElementKind::ListItem { .. }))
            .expect("list item should be present");
        assert_eq!(list_item.page, Some(3));
    }

    /// Full round-trip through PptxExtractor::extract_bytes → derive_extraction_result →
    /// ExtractedDocument.pages, asserting that speaker_notes and section_name are present.
    #[tokio::test]
    async fn test_extract_bytes_populates_speaker_notes_and_section_name() {
        use crate::core::config::{ExtractionConfig, PageConfig};
        use crate::extraction::derive::derive_extraction_result;
        use crate::plugins::InternalDocumentExtractor;

        let pptx = crate::extraction::pptx::tests::create_pptx_with_sections_and_notes(
            &[
                ("Title", Some("Intro notes.")),
                ("Body", Some("Body notes.")),
                ("End", None),
            ],
            &[("Chapter 1", &[1, 2]), ("Chapter 2", &[3])],
        );

        let extractor = PptxExtractor::new();
        let config = ExtractionConfig {
            pages: Some(PageConfig::default()),
            ..Default::default()
        };
        let mime = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
        let internal_doc = extractor
            .extract_content(&pptx, mime, &config)
            .await
            .expect("extraction failed");
        let result = derive_extraction_result(internal_doc, true, crate::core::config::OutputFormat::Plain);

        assert!(
            !result.content.contains("<!-- Slide number:"),
            "internal slide markers must not leak into rendered output"
        );
        let pages = result.pages.as_ref().expect("pages should be populated");
        assert_eq!(pages.len(), 3);

        assert_eq!(pages[0].speaker_notes.as_deref(), Some("Intro notes."));
        assert_eq!(pages[0].section_name.as_deref(), Some("Chapter 1"));

        assert_eq!(pages[1].speaker_notes.as_deref(), Some("Body notes."));
        assert_eq!(pages[1].section_name.as_deref(), Some("Chapter 1"));

        assert!(pages[2].speaker_notes.is_none());
        assert_eq!(pages[2].section_name.as_deref(), Some("Chapter 2"));
    }
}
