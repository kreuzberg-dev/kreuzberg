//! Djot document extractor with plugin integration.
//!
//! Implements the DocumentExtractor and Plugin traits for Djot markup files.

use super::super::annotation_utils::adjust_annotations_for_trim;
use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::Metadata;
use crate::types::internal::InternalDocument;
use crate::types::internal::{RelationshipKind, RelationshipTarget};
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::uri::{ExtractedUri, classify_uri};
use async_trait::async_trait;
use jotdown::{Container, Event, Parser};
#[cfg_attr(alef, alef(skip))]
/// Djot markup extractor with metadata and table support.
///
/// Parses Djot documents with YAML frontmatter, extracting:
/// - Metadata from YAML frontmatter
/// - Plain text content
/// - Tables as structured data
/// - Document structure (headings, links, code blocks)
#[derive(Debug, Clone)]
pub struct DjotExtractor;

impl DjotExtractor {
    /// Create a new Djot extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl DjotExtractor {
    /// Build an `InternalDocument` from jotdown events.
    pub(crate) fn build_internal_document(events: &[Event]) -> InternalDocument {
        use crate::types::builder;
        use crate::types::document_structure::TextAnnotation;

        let mut b = InternalDocumentBuilder::new("djot");

        let mut paragraph_text = String::new();
        let mut paragraph_annotations: Vec<TextAnnotation> = Vec::new();
        let mut in_paragraph = false;
        let mut heading_text = String::new();
        let mut heading_annotations: Vec<TextAnnotation> = Vec::new();
        let mut heading_level: u8 = 0;
        let mut in_heading = false;
        let mut code_text = String::new();
        let mut code_lang: Option<String> = None;
        let mut in_code_block = false;
        let mut in_math = false;
        let mut math_text = String::new();
        let mut list_stack: Vec<bool> = Vec::new();
        let mut list_item_text = String::new();
        let mut list_item_annotations: Vec<TextAnnotation> = Vec::new();
        let mut in_list_item = false;
        let mut in_raw_block = false;
        let mut raw_format: Option<String> = None;
        let mut raw_text = String::new();
        let mut in_verbatim = false;
        let mut verbatim_start: u32 = 0;
        let mut in_image = false;
        let mut image_alt = String::new();
        let mut in_footnote = false;
        let mut footnote_label = String::new();
        let mut footnote_text = String::new();
        let mut table_rows: Vec<Vec<String>> = Vec::new();
        let mut table_row: Vec<String> = Vec::new();
        let mut table_cell = String::new();
        let mut in_table_cell = false;
        let mut in_description_term = false;
        let mut description_term_text = String::new();
        let mut in_description_details = false;
        let mut description_details_text = String::new();
        // Index of the image element that is the sole leading content of the
        // current paragraph, used to detect the djot "figure with caption"
        // pattern: an image followed immediately (no blank line) by text.
        let mut figure_image_index: Option<u32> = None;
        let mut figure_disqualified = false;

        let mut annotation_starts: Vec<(u8, u32, Option<String>)> = Vec::new();

        for event in events {
            match event {
                Event::Start(Container::Heading { level, .. }, _) => {
                    heading_text.clear();
                    heading_annotations.clear();
                    annotation_starts.clear();
                    heading_level = *level as u8;
                    in_heading = true;
                }
                Event::End(Container::Heading { .. }) => {
                    in_heading = false;
                    let text = heading_text.trim().to_string();
                    if !text.is_empty() {
                        let annotations =
                            adjust_annotations_for_trim(std::mem::take(&mut heading_annotations), &heading_text, &text);
                        let idx = b.push_heading(heading_level, &text, None, None);
                        if !annotations.is_empty() {
                            b.set_annotations(idx, annotations);
                        }
                    }
                    heading_text.clear();
                    heading_annotations.clear();
                }
                Event::Start(Container::Paragraph, _)
                    if !in_heading && !in_list_item && !in_description_term && !in_description_details =>
                {
                    paragraph_text.clear();
                    paragraph_annotations.clear();
                    in_paragraph = true;
                    figure_image_index = None;
                    figure_disqualified = false;
                }
                Event::End(Container::Paragraph) => {
                    if in_paragraph {
                        in_paragraph = false;
                        let text = paragraph_text.trim().to_string();
                        if !text.is_empty() {
                            let annotations = adjust_annotations_for_trim(
                                std::mem::take(&mut paragraph_annotations),
                                &paragraph_text,
                                &text,
                            );
                            let idx = b.push_paragraph(&text, annotations, None, None);
                            if let Some(image_idx) = figure_image_index
                                && !figure_disqualified
                            {
                                b.push_relationship(
                                    idx,
                                    RelationshipTarget::Index(image_idx),
                                    RelationshipKind::Caption,
                                );
                            }
                        }
                        paragraph_text.clear();
                        paragraph_annotations.clear();
                        figure_image_index = None;
                        figure_disqualified = false;
                    } else if in_list_item {
                    }
                }
                Event::Start(Container::Strong, _) => {
                    if in_paragraph {
                        annotation_starts.push((0, paragraph_text.len() as u32, None));
                    } else if in_heading {
                        annotation_starts.push((0, heading_text.len() as u32, None));
                    } else if in_list_item {
                        annotation_starts.push((0, list_item_text.len() as u32, None));
                    }
                }
                Event::End(Container::Strong) => {
                    if let Some(pos) = annotation_starts.iter().rposition(|(k, _, _)| *k == 0) {
                        let (_, start, _) = annotation_starts.remove(pos);
                        if in_paragraph {
                            let end = paragraph_text.len() as u32;
                            if start < end {
                                paragraph_annotations.push(builder::bold(start, end));
                            }
                        } else if in_heading {
                            let end = heading_text.len() as u32;
                            if start < end {
                                heading_annotations.push(builder::bold(start, end));
                            }
                        } else if in_list_item {
                            let end = list_item_text.len() as u32;
                            if start < end {
                                list_item_annotations.push(builder::bold(start, end));
                            }
                        }
                    }
                }
                Event::Start(Container::Emphasis, _) => {
                    if in_paragraph {
                        annotation_starts.push((1, paragraph_text.len() as u32, None));
                    } else if in_heading {
                        annotation_starts.push((1, heading_text.len() as u32, None));
                    } else if in_list_item {
                        annotation_starts.push((1, list_item_text.len() as u32, None));
                    }
                }
                Event::End(Container::Emphasis) => {
                    if let Some(pos) = annotation_starts.iter().rposition(|(k, _, _)| *k == 1) {
                        let (_, start, _) = annotation_starts.remove(pos);
                        if in_paragraph {
                            let end = paragraph_text.len() as u32;
                            if start < end {
                                paragraph_annotations.push(builder::italic(start, end));
                            }
                        } else if in_heading {
                            let end = heading_text.len() as u32;
                            if start < end {
                                heading_annotations.push(builder::italic(start, end));
                            }
                        } else if in_list_item {
                            let end = list_item_text.len() as u32;
                            if start < end {
                                list_item_annotations.push(builder::italic(start, end));
                            }
                        }
                    }
                }
                Event::Start(Container::Delete, _) => {
                    if in_paragraph {
                        annotation_starts.push((2, paragraph_text.len() as u32, None));
                    } else if in_heading {
                        annotation_starts.push((2, heading_text.len() as u32, None));
                    } else if in_list_item {
                        annotation_starts.push((2, list_item_text.len() as u32, None));
                    }
                }
                Event::End(Container::Delete) => {
                    if let Some(pos) = annotation_starts.iter().rposition(|(k, _, _)| *k == 2) {
                        let (_, start, _) = annotation_starts.remove(pos);
                        if in_paragraph {
                            let end = paragraph_text.len() as u32;
                            if start < end {
                                paragraph_annotations.push(builder::strikethrough(start, end));
                            }
                        } else if in_heading {
                            let end = heading_text.len() as u32;
                            if start < end {
                                heading_annotations.push(builder::strikethrough(start, end));
                            }
                        } else if in_list_item {
                            let end = list_item_text.len() as u32;
                            if start < end {
                                list_item_annotations.push(builder::strikethrough(start, end));
                            }
                        }
                    }
                }
                Event::Start(Container::Verbatim, _) => {
                    if in_paragraph {
                        in_verbatim = true;
                        verbatim_start = paragraph_text.len() as u32;
                    } else if in_heading {
                        in_verbatim = true;
                        verbatim_start = heading_text.len() as u32;
                    } else if in_list_item {
                        in_verbatim = true;
                        verbatim_start = list_item_text.len() as u32;
                    }
                }
                Event::End(Container::Verbatim) if in_verbatim => {
                    in_verbatim = false;
                    if in_paragraph {
                        let end = paragraph_text.len() as u32;
                        if verbatim_start < end {
                            paragraph_annotations.push(builder::code(verbatim_start, end));
                        }
                    } else if in_heading {
                        let end = heading_text.len() as u32;
                        if verbatim_start < end {
                            heading_annotations.push(builder::code(verbatim_start, end));
                        }
                    } else if in_list_item {
                        let end = list_item_text.len() as u32;
                        if verbatim_start < end {
                            list_item_annotations.push(builder::code(verbatim_start, end));
                        }
                    }
                }
                Event::Start(Container::Link(url, _), _) => {
                    if in_paragraph {
                        annotation_starts.push((4, paragraph_text.len() as u32, Some(url.to_string())));
                    } else if in_heading {
                        annotation_starts.push((4, heading_text.len() as u32, Some(url.to_string())));
                    } else if in_list_item {
                        annotation_starts.push((4, list_item_text.len() as u32, Some(url.to_string())));
                    }
                }
                Event::End(Container::Link(..)) => {
                    if let Some(pos) = annotation_starts.iter().rposition(|(k, _, _)| *k == 4) {
                        let (_, start, url_opt) = annotation_starts.remove(pos);
                        if let Some(url) = url_opt {
                            let label_text = if in_paragraph {
                                let end = paragraph_text.len() as u32;
                                if start < end {
                                    paragraph_annotations.push(builder::link(start, end, &url, None));
                                    Some(paragraph_text[start as usize..end as usize].to_string())
                                } else {
                                    None
                                }
                            } else if in_heading {
                                let end = heading_text.len() as u32;
                                if start < end {
                                    heading_annotations.push(builder::link(start, end, &url, None));
                                    Some(heading_text[start as usize..end as usize].to_string())
                                } else {
                                    None
                                }
                            } else if in_list_item {
                                let end = list_item_text.len() as u32;
                                if start < end {
                                    list_item_annotations.push(builder::link(start, end, &url, None));
                                    Some(list_item_text[start as usize..end as usize].to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if !url.is_empty() {
                                let kind = classify_uri(&url);
                                b.push_uri(ExtractedUri {
                                    url,
                                    label: label_text.filter(|s| !s.is_empty()),
                                    page: None,
                                    kind,
                                });
                            }
                        }
                    }
                }
                Event::Start(Container::CodeBlock { language }, _) => {
                    code_text.clear();
                    code_lang = if language.is_empty() {
                        None
                    } else {
                        Some(language.to_string())
                    };
                    in_code_block = true;
                }
                Event::End(Container::CodeBlock { .. }) => {
                    in_code_block = false;
                    let text = code_text.trim_end().to_string();
                    if !text.is_empty() {
                        b.push_code(&text, code_lang.as_deref(), None, None);
                    }
                    code_text.clear();
                    code_lang = None;
                }
                Event::Start(Container::RawBlock { format }, _) => {
                    in_raw_block = true;
                    raw_format = Some(format.to_string());
                    raw_text.clear();
                }
                Event::End(Container::RawBlock { .. }) => {
                    in_raw_block = false;
                    let text = raw_text.trim().to_string();
                    if !text.is_empty() {
                        b.push_raw_block(raw_format.as_deref().unwrap_or("unknown"), &text, None);
                    }
                    raw_text.clear();
                    raw_format = None;
                }
                Event::Start(Container::Blockquote, _) => {
                    b.push_quote_start();
                }
                Event::End(Container::Blockquote) => {
                    b.push_quote_end();
                }
                Event::Start(Container::Div { class }, _) => {
                    let label = if class.is_empty() { None } else { Some(class.as_ref()) };
                    b.push_group_start(label, None);
                }
                Event::End(Container::Div { .. }) => {
                    b.push_group_end();
                }
                Event::Start(Container::DescriptionTerm, _) => {
                    in_description_term = true;
                    description_term_text.clear();
                }
                Event::End(Container::DescriptionTerm) => {
                    in_description_term = false;
                    let text = description_term_text.trim().to_string();
                    if !text.is_empty() {
                        b.push_definition_term(&text, None);
                    }
                    description_term_text.clear();
                }
                Event::Start(Container::DescriptionDetails, _) => {
                    in_description_details = true;
                    description_details_text.clear();
                }
                Event::End(Container::DescriptionDetails) => {
                    in_description_details = false;
                    let text = description_details_text.trim().to_string();
                    if !text.is_empty() {
                        b.push_definition_description(&text, None);
                    }
                    description_details_text.clear();
                }
                Event::Start(Container::List { kind, .. }, _) => {
                    let ordered = matches!(kind, jotdown::ListKind::Ordered { .. });
                    b.push_list(ordered);
                    list_stack.push(ordered);
                }
                Event::End(Container::List { .. }) if list_stack.pop().is_some() => {
                    b.end_list();
                }
                Event::Start(Container::ListItem | Container::TaskListItem { .. }, _) => {
                    list_item_text.clear();
                    list_item_annotations.clear();
                    annotation_starts.clear();
                    in_list_item = true;
                }
                Event::End(Container::ListItem | Container::TaskListItem { .. }) => {
                    in_list_item = false;
                    let text = list_item_text.trim().to_string();
                    if let Some(ordered) = list_stack.last().copied()
                        && !text.is_empty()
                    {
                        let annotations = adjust_annotations_for_trim(
                            std::mem::take(&mut list_item_annotations),
                            &list_item_text,
                            &text,
                        );
                        b.push_list_item(&text, ordered, annotations, None, None);
                    }
                    list_item_text.clear();
                    list_item_annotations.clear();
                }
                Event::Start(Container::Math { display }, _) => {
                    if *display {
                        in_math = true;
                        math_text.clear();
                    } else if in_paragraph {
                        paragraph_text.push('$');
                    } else if in_heading {
                        heading_text.push('$');
                    } else if in_list_item {
                        list_item_text.push('$');
                    } else if in_description_term {
                        description_term_text.push('$');
                    } else if in_description_details {
                        description_details_text.push('$');
                    }
                }
                Event::End(Container::Math { display }) => {
                    if *display {
                        in_math = false;
                        let text = math_text.trim().to_string();
                        if !text.is_empty() {
                            b.push_formula(&text, None, None);
                        }
                        math_text.clear();
                    } else if in_paragraph {
                        paragraph_text.push('$');
                    } else if in_heading {
                        heading_text.push('$');
                    } else if in_list_item {
                        list_item_text.push('$');
                    } else if in_description_term {
                        description_term_text.push('$');
                    } else if in_description_details {
                        description_details_text.push('$');
                    }
                }
                Event::Start(Container::Image(..), _) => {
                    in_image = true;
                    image_alt.clear();
                }
                Event::End(Container::Image(src, ..)) => {
                    in_image = false;
                    use crate::types::document_structure::ContentLayer;
                    use crate::types::internal::{ElementKind, InternalElement, InternalElementId};
                    let alt = image_alt.trim().to_string();
                    let kind = ElementKind::Image { image_index: u32::MAX };
                    let id = InternalElementId::generate(kind.discriminant(), &alt, None, 0);
                    let image_element_index = b.push_element(InternalElement {
                        id,
                        kind,
                        text: alt,
                        depth: 0,
                        page: None,
                        bbox: None,
                        layer: ContentLayer::Body,
                        annotations: Vec::new(),
                        attributes: None,
                        anchor: None,
                        ocr_geometry: None,
                        ocr_confidence: None,
                        ocr_rotation: None,
                    });
                    // Track djot's figure-with-caption pattern: an image that is the sole
                    // leading content of a paragraph, optionally followed (no blank line)
                    // by caption text handled when the paragraph ends.
                    if in_paragraph {
                        if figure_image_index.is_none() && !figure_disqualified && paragraph_text.trim().is_empty() {
                            figure_image_index = Some(image_element_index);
                        } else {
                            figure_disqualified = true;
                        }
                    }
                    let src_str: &str = src.as_ref();
                    if !src_str.is_empty() {
                        let trimmed = image_alt.trim();
                        let label = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        };
                        b.push_uri(ExtractedUri::image(src_str, label));
                    }
                    image_alt.clear();
                }
                Event::Start(Container::Footnote { label }, _) => {
                    in_footnote = true;
                    footnote_label = label.to_string();
                    footnote_text.clear();
                }
                Event::End(Container::Footnote { .. }) if in_footnote => {
                    in_footnote = false;
                    let text = footnote_text.trim().to_string();
                    if !text.is_empty() {
                        b.push_footnote_definition(&text, &footnote_label, None);
                    }
                    footnote_text.clear();
                    footnote_label.clear();
                }
                Event::FootnoteReference(name) => {
                    b.push_footnote_ref(name, name, None);
                }
                Event::Start(Container::Table, _) => {
                    table_rows.clear();
                }
                Event::Start(Container::TableRow { .. }, _) => {
                    table_row.clear();
                }
                Event::Start(Container::TableCell { .. }, _) => {
                    table_cell.clear();
                    in_table_cell = true;
                }
                Event::End(Container::TableCell { .. }) if in_table_cell => {
                    in_table_cell = false;
                    table_row.push(std::mem::take(&mut table_cell).trim().to_string());
                }
                Event::End(Container::TableRow { .. }) if !table_row.is_empty() => {
                    table_rows.push(std::mem::take(&mut table_row));
                }
                Event::End(Container::Table) if !table_rows.is_empty() => {
                    b.push_table_from_cells(&std::mem::take(&mut table_rows), None, None);
                }
                Event::Str(s) => {
                    if in_table_cell {
                        table_cell.push_str(s);
                    } else if in_image {
                        image_alt.push_str(s);
                    } else if in_footnote {
                        footnote_text.push_str(s);
                    } else if in_code_block {
                        code_text.push_str(s);
                    } else if in_raw_block {
                        raw_text.push_str(s);
                    } else if in_math {
                        math_text.push_str(s);
                    } else if in_heading {
                        heading_text.push_str(s);
                    } else if in_list_item {
                        list_item_text.push_str(s);
                    } else if in_description_term {
                        description_term_text.push_str(s);
                    } else if in_description_details {
                        description_details_text.push_str(s);
                    } else if in_paragraph {
                        paragraph_text.push_str(s);
                    }
                }
                Event::Softbreak => {
                    if in_code_block {
                        code_text.push('\n');
                    } else if in_heading {
                        heading_text.push(' ');
                    } else if in_list_item {
                        list_item_text.push(' ');
                    } else if in_description_term {
                        description_term_text.push(' ');
                    } else if in_description_details {
                        description_details_text.push(' ');
                    } else if in_paragraph {
                        paragraph_text.push(' ');
                    }
                }
                Event::Hardbreak => {
                    if in_code_block {
                        code_text.push('\n');
                    } else if in_paragraph {
                        paragraph_text.push('\n');
                    }
                }
                _ => {}
            }
        }

        b.build()
    }
}

impl Default for DjotExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for DjotExtractor {
    fn name(&self) -> &str {
        "djot-extractor"
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
        "Extracts content from Djot markup files with YAML frontmatter and table support"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for DjotExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let _ = config;
        let text = String::from_utf8_lossy(content).into_owned();

        let (yaml, remaining_content, frontmatter_warning) =
            crate::extractors::frontmatter_utils::extract_frontmatter_with_warning(&text);

        let mut metadata = if let Some(ref yaml_value) = yaml {
            crate::extractors::frontmatter_utils::extract_metadata_from_yaml(yaml_value)
        } else {
            Metadata::default()
        };

        if metadata.title.is_none()
            && let Some(title) = crate::extractors::frontmatter_utils::extract_title_from_content(&remaining_content)
        {
            metadata.title = Some(title);
        }

        let parser = Parser::new(&remaining_content);
        let events: Vec<Event> = parser.collect();

        let mut doc = Self::build_internal_document(&events);
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;
        doc.processing_warnings.extend(frontmatter_warning);

        Ok(doc)
    }

    async fn extract_path(
        &self,
        path: &std::path::Path,
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        crate::core::path_resolver::extract_file_with_image_resolution(self, path, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/djot", "text/x-djot"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_djot_extractor_creation() {
        let extractor = DjotExtractor::new();
        assert_eq!(extractor.name(), "djot-extractor");
    }

    #[test]
    fn test_can_extract_djot_mime_types() {
        let extractor = DjotExtractor::new();
        let mime_types = extractor.supported_mime_types();

        assert!(mime_types.contains(&"text/djot"));
        assert!(mime_types.contains(&"text/x-djot"));
    }

    #[test]
    fn test_plugin_interface() {
        let extractor = DjotExtractor::new();
        assert_eq!(extractor.author(), "Xberg Team");
        assert!(!extractor.version().is_empty());
        assert!(!extractor.description().is_empty());
    }

    #[tokio::test]
    async fn test_extract_simple_djot() {
        let content =
            b"# Header\n\nThis is a paragraph with *bold* and _italic_ text.\n\n## Subheading\n\nMore content here.";
        let extractor = DjotExtractor::new();
        let config = ExtractionConfig::default();

        let result = extractor.extract_content(content, "text/djot", &config).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(result.content.contains("Header"));
        assert!(result.content.contains("This is a paragraph"));
        assert!(result.content.contains("bold"));
        assert!(result.content.contains("italic"));
    }

    /// Regression test for the bug where extracted tables never reached rendered output:
    /// `extract_content` used to extract tables via a separate `extract_tables_from_events`
    /// pass and re-push them with the raw, element-less `InternalDocument::push_table`, which
    /// only records the table data without creating a matching `ElementKind::Table` element.
    /// Since every renderer walks `doc.elements`, the table content was silently dropped. Tables
    /// are now parsed in place inside `build_internal_document`, preserving both their document
    /// position and producing a proper element.
    #[tokio::test]
    async fn test_djot_tables_render_in_output() {
        let content = b"Intro paragraph.\n\n| Name | Age |\n|------|-----|\n| Alice | 30 |\n\nOutro paragraph.";
        let extractor = DjotExtractor::new();
        let config = ExtractionConfig::default();

        let doc = extractor
            .extract_content(content, "text/djot", &config)
            .await
            .expect("extraction should succeed");

        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.tables[0].cells[0], vec!["Name", "Age"]);
        assert_eq!(doc.tables[0].cells[1], vec!["Alice", "30"]);

        let table_element_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Table { .. }))
            .count();
        assert_eq!(
            table_element_count, 1,
            "expected exactly one Table element in {:?}",
            doc.elements
        );

        let result =
            crate::extraction::derive::derive_extraction_result(doc, true, crate::core::config::OutputFormat::Markdown);
        assert!(
            result.content.contains("Alice") && result.content.contains("30"),
            "table content missing from rendered output: {}",
            result.content
        );
        let intro_pos = result.content.find("Intro paragraph").expect("intro present");
        let table_pos = result.content.find("Alice").expect("table content present");
        let outro_pos = result.content.find("Outro paragraph").expect("outro present");
        assert!(
            intro_pos < table_pos && table_pos < outro_pos,
            "table should be positioned in document flow: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_trimmed_paragraph_with_emoji_djot() {
        let djot = "  *bold* \u{1F389} text  ".as_bytes();
        let extractor = DjotExtractor::new();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(djot, "text/djot", &config)
            .await
            .expect("Should handle emoji in trimmed djot paragraph");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("bold"), "Bold text preserved");
        assert!(result.content.contains("\u{1F389}"), "Emoji preserved after trim");
    }

    #[tokio::test]
    async fn test_cjk_paragraph_with_formatting_djot() {
        let djot = "# CJK\n\nこれは*太字*テスト".as_bytes();
        let extractor = DjotExtractor::new();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(djot, "text/djot", &config)
            .await
            .expect("Should handle CJK with bold formatting");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("太字"), "Bold CJK content present");
        assert!(result.content.contains("これは"), "Leading CJK preserved");
    }

    #[tokio::test]
    async fn test_image_uri_extraction_djot() {
        let djot = b"![A diagram](https://example.com/diagram.png)\n\nSome text.";
        let extractor = DjotExtractor::new();
        let config = ExtractionConfig::default();

        let doc = extractor
            .extract_content(djot, "text/djot", &config)
            .await
            .expect("image djot should extract");

        let has_image_uri = doc.uris.iter().any(|u| u.url.contains("diagram.png"));
        assert!(has_image_uri, "image URI should be captured from Djot image node");
    }
}
