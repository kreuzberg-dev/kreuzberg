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
    /// Parses each archive-derived slide independently so page metadata never
    /// depends on headings or marker-like user text.
    ///
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

    /// Build the delimited forms of a deck's math runs, longest first.
    ///
    /// A math run reaches markdown text as `$$latex$$` (display) or `$latex$`
    /// (inline). Matching the exact strings the OMML converter produced keeps
    /// author text that merely holds a `$` out of the formula list.
    fn math_forms(formulas: &[(String, bool)]) -> Vec<(String, String)> {
        let mut forms: Vec<(String, String)> = formulas
            .iter()
            .map(|(latex, is_display)| {
                let delimiter = if *is_display { "$$" } else { "$" };
                (format!("{delimiter}{latex}{delimiter}"), latex.clone())
            })
            .collect();
        forms.sort_by_key(|form| std::cmp::Reverse(form.0.len()));
        forms
    }

    /// Pull the math spans out of one line of extracted text.
    ///
    /// Returns the line without its math and the LaTeX of every span removed, in
    /// the order the spans appeared.
    ///
    /// Plain text carries no delimiters, so there a line that is exactly one
    /// formula's LaTeX becomes that formula. This covers the equation shape a
    /// slide deck usually holds. Math mixed into a line of plain text stays in
    /// the line, because the LaTeX and the words around it are the same
    /// characters. Markdown text keeps its delimiters, so both shapes of math
    /// come out of it.
    fn split_line_math<'a>(
        line: &'a str,
        forms: &[(String, String)],
        formulas: &[(String, bool)],
        plain_output: bool,
    ) -> (Cow<'a, str>, Vec<String>) {
        if formulas.is_empty() {
            return (Cow::Borrowed(line), Vec::new());
        }
        if plain_output {
            let trimmed = line.trim();
            return match formulas.iter().find(|(latex, _)| latex.as_str() == trimmed) {
                Some((latex, _)) => (Cow::Borrowed(""), vec![latex.clone()]),
                None => (Cow::Borrowed(line), Vec::new()),
            };
        }
        if !line.contains('$') {
            return (Cow::Borrowed(line), Vec::new());
        }

        let mut rest = line;
        let mut text = String::new();
        let mut found: Vec<String> = Vec::new();
        loop {
            let earliest = forms
                .iter()
                .filter_map(|form| rest.find(form.0.as_str()).map(|pos| (pos, form)))
                .min_by_key(|(pos, _)| *pos);
            let Some((pos, form)) = earliest else {
                break;
            };
            text.push_str(&rest[..pos]);
            found.push(form.1.clone());
            rest = &rest[pos + form.0.len()..];
        }
        if found.is_empty() {
            return (Cow::Borrowed(line), Vec::new());
        }
        text.push_str(rest);

        (Cow::Owned(text.split_whitespace().collect::<Vec<_>>().join(" ")), found)
    }

    /// Emit a formula element per LaTeX string, in order.
    fn push_line_formulas(
        builder: &mut InternalDocumentBuilder,
        formulas: &[String],
        slide_num: u32,
        budget: &mut SecurityBudget,
    ) -> Result<()> {
        for latex in formulas {
            budget.account_text(latex.len())?;
            builder.push_formula(latex, Some(slide_num), None);
        }
        Ok(())
    }

    fn build_internal_document(
        slide_contents: &[(u32, String)],
        slide_count: u32,
        formulas: &[(String, bool)],
        plain_output: bool,
        budget: &mut SecurityBudget,
    ) -> Result<InternalDocument> {
        let mut builder = InternalDocumentBuilder::new("pptx");
        let mut saw_title = false;
        let forms = Self::math_forms(formulas);

        for (slide_num, content) in slide_contents {
            let mut in_notes = false;

            for block in content.split("\n\n") {
                budget.step()?;
                let trimmed = block.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed.starts_with("### Notes:") || trimmed == "Notes:" {
                    in_notes = true;
                    continue;
                }

                if let Some(title_text) = trimmed.strip_prefix("# ") {
                    in_notes = false;
                    saw_title = true;
                    let (title_text, title_formulas) = Self::split_line_math(title_text, &forms, formulas, plain_output);
                    Self::push_line_formulas(&mut builder, &title_formulas, *slide_num, budget)?;
                    let title = title_text.trim();
                    if !title.is_empty() {
                        budget.account_text(title.len())?;
                        builder.push_heading(2, title, Some(*slide_num), None);
                    }
                    continue;
                }

                if in_notes {
                    in_notes = false;
                }

                if trimmed.starts_with('|') {
                    let cells = Self::parse_markdown_table(trimmed);
                    if !cells.is_empty() {
                        builder.push_table_from_cells(&cells, Some(*slide_num), None);
                    }
                    continue;
                }

                let mut in_list: Option<bool> = None;

                for line in trimmed.lines() {
                    let (line_text, line_formulas) = Self::split_line_math(line, &forms, formulas, plain_output);
                    let lt = line_text.trim();
                    if lt.is_empty() && line_formulas.is_empty() {
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
                        // A list item's math becomes its own element ahead of the
                        // item, the order DOCX uses for math runs in a paragraph.
                        Self::push_line_formulas(&mut builder, &line_formulas, *slide_num, budget)?;
                        budget.account_text(item_text.len())?;
                        builder.push_list_item(item_text, ordered, vec![], Some(*slide_num), None);
                    } else {
                        if in_list.is_some() {
                            builder.end_list();
                            in_list = None;
                        }
                        Self::push_line_formulas(&mut builder, &line_formulas, *slide_num, budget)?;
                        if !lt.is_empty() {
                            budget.account_text(lt.len())?;
                            builder.push_paragraph(lt, vec![], Some(*slide_num), None);
                        }
                    }
                }

                if in_list.is_some() {
                    builder.end_list();
                }
            }
        }

        // Preserve the legacy all-untitled output shape: it contains one Slide
        // sentinel, while titled decks do not gain new thematic-break/JSON nodes.
        if !saw_title && slide_count > 0 {
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
        slide_contents: &[(u32, String)],
        formulas: &[(String, bool)],
        plain_output: bool,
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

        let mut doc = Self::build_internal_document(
            slide_contents,
            pptx_result.slide_count as u32,
            formulas,
            plain_output,
            budget,
        )?;
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

        let pptx_internal = {
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
                        let result = crate::extraction::pptx::extract_pptx_from_bytes_with_slide_contents(
                            &content_owned,
                            &options,
                            &mut warnings,
                        );
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
                    crate::extraction::pptx::extract_pptx_from_bytes_with_slide_contents(
                        content,
                        &options,
                        &mut pptx_warnings,
                    )?
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
                crate::extraction::pptx::extract_pptx_from_bytes_with_slide_contents(
                    content,
                    &options,
                    &mut pptx_warnings,
                )?
            }
        };

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = Self::build_document_from_result(
            pptx_internal.result,
            &pptx_internal.slide_contents,
            &pptx_internal.formulas,
            pptx_internal.plain_output,
            mime_type,
            extract_images,
            &mut budget,
        )?;
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
        let pptx_internal = crate::extraction::pptx::extract_pptx_from_path_with_slide_contents(
            path_str,
            &options,
            &mut pptx_warnings,
        )?;

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = Self::build_document_from_result(
            pptx_internal.result,
            &pptx_internal.slide_contents,
            &pptx_internal.formulas,
            pptx_internal.plain_output,
            mime_type,
            extract_images,
            &mut budget,
        )?;
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

    /// A slide with math: the LaTeX must reach `ExtractedDocument.formulas`, not
    /// only the text. The deck holds display math in its own shape, inline math
    /// beside text, and a `$` amount that is not math at all.
    #[tokio::test]
    async fn test_slide_math_populates_formulas() {
        use crate::core::config::ExtractionConfig;
        use crate::extraction::derive::derive_extraction_result;
        use crate::plugins::InternalDocumentExtractor;

        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
       xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main"
       xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
    <p:cSld><p:spTree>
        <p:sp><p:txBody>
            <a:p><a:r><a:t>Budget is $5 per unit</a:t></a:r></a:p>
        </p:txBody></p:sp>
        <p:sp><p:txBody>
            <a:p>
                <mc:AlternateContent>
                    <mc:Choice Requires="a14"><a14:m>
                        <m:oMathPara><m:oMath><m:sSup>
                            <m:e><m:r><m:t>x</m:t></m:r></m:e>
                            <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                        </m:sSup></m:oMath></m:oMathPara>
                    </a14:m></mc:Choice>
                    <mc:Fallback><a:r><a:t>[equation]</a:t></a:r></mc:Fallback>
                </mc:AlternateContent>
            </a:p>
            <a:p>
                <a:r><a:t>Rate </a:t></a:r>
                <mc:AlternateContent>
                    <mc:Choice Requires="a14"><a14:m>
                        <m:oMath><m:r><m:t>a</m:t></m:r></m:oMath>
                    </a14:m></mc:Choice>
                    <mc:Fallback><a:r><a:t>[a]</a:t></a:r></mc:Fallback>
                </mc:AlternateContent>
                <a:r><a:t> per hour</a:t></a:r>
            </a:p>
        </p:txBody></p:sp>
    </p:spTree></p:cSld>
</p:sld>"#;

        let pptx = crate::extraction::pptx::tests::build_single_slide_pptx(slide_xml, None, &[]);
        let extractor = PptxExtractor::new();
        let mime = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let internal_doc = extractor
            .extract_content(&pptx, mime, &config)
            .await
            .expect("extraction failed");
        let result = derive_extraction_result(internal_doc, false, crate::core::config::OutputFormat::Markdown);

        let latex: Vec<&str> = result.formulas.iter().map(|f| f.latex.as_str()).collect();
        assert_eq!(latex, vec!["x^{2}", "a"], "both math runs reach formulas");
        assert!(
            result.content.contains("Budget is $5 per unit"),
            "a dollar amount stays text, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Rate per hour"),
            "the text around inline math survives, got: {:?}",
            result.content
        );
    }

    /// Plain output carries no math delimiters, so a shape that holds nothing but
    /// math is still recognized by its exact LaTeX. Math mixed into a line of text
    /// stays in that line.
    #[tokio::test]
    async fn test_standalone_slide_math_populates_formulas_in_plain_output() {
        use crate::core::config::ExtractionConfig;
        use crate::extraction::derive::derive_extraction_result;
        use crate::plugins::InternalDocumentExtractor;

        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
       xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main"
       xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
    <p:cSld><p:spTree>
        <p:sp><p:txBody>
            <a:p><a:r><a:t>Energy of a body at rest</a:t></a:r></a:p>
        </p:txBody></p:sp>
        <p:sp><p:txBody>
            <a:p>
                <mc:AlternateContent>
                    <mc:Choice Requires="a14"><a14:m>
                        <m:oMathPara><m:oMath><m:sSup>
                            <m:e><m:r><m:t>x</m:t></m:r></m:e>
                            <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                        </m:sSup></m:oMath></m:oMathPara>
                    </a14:m></mc:Choice>
                    <mc:Fallback><a:r><a:t>[equation]</a:t></a:r></mc:Fallback>
                </mc:AlternateContent>
            </a:p>
        </p:txBody></p:sp>
    </p:spTree></p:cSld>
</p:sld>"#;

        let pptx = crate::extraction::pptx::tests::build_single_slide_pptx(slide_xml, None, &[]);
        let extractor = PptxExtractor::new();
        let mime = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
        let internal_doc = extractor
            .extract_content(&pptx, mime, &ExtractionConfig::default())
            .await
            .expect("extraction failed");
        let result = derive_extraction_result(internal_doc, false, crate::core::config::OutputFormat::Plain);

        let latex: Vec<&str> = result.formulas.iter().map(|f| f.latex.as_str()).collect();
        assert_eq!(latex, vec!["x^{2}"]);
    }

    #[test]
    fn test_split_line_math_pulls_delimited_spans() {
        let formulas = vec![("x^{2}".to_string(), true), ("a".to_string(), false)];
        let forms = PptxExtractor::math_forms(&formulas);

        let (text, found) = PptxExtractor::split_line_math("Rate $a$ per hour", &forms, &formulas, false);
        assert_eq!(text, "Rate per hour");
        assert_eq!(found, vec!["a".to_string()]);

        let (text, found) = PptxExtractor::split_line_math("$$x^{2}$$", &forms, &formulas, false);
        assert_eq!(text, "");
        assert_eq!(found, vec!["x^{2}".to_string()]);
    }

    #[test]
    fn test_split_line_math_keeps_plain_dollar_text() {
        let formulas = vec![("a".to_string(), false)];
        let forms = PptxExtractor::math_forms(&formulas);

        let (text, found) = PptxExtractor::split_line_math("Budget is $5 per unit", &forms, &formulas, false);
        assert_eq!(text, "Budget is $5 per unit");
        assert!(found.is_empty(), "author text with a dollar sign is not math");
    }

    #[test]
    fn test_split_line_math_matches_undelimited_plain_output() {
        let formulas = vec![("x^{2}".to_string(), true)];
        let forms = PptxExtractor::math_forms(&formulas);

        let (text, found) = PptxExtractor::split_line_math("x^{2}", &forms, &formulas, true);
        assert_eq!(text, "");
        assert_eq!(found, vec!["x^{2}".to_string()], "plain output carries no delimiters");
    }

    /// Markdown text keeps its delimiters, so a line that merely repeats a
    /// formula's characters is author text, not math.
    #[test]
    fn test_split_line_math_leaves_undelimited_line_in_markdown_text() {
        let formulas = vec![("n".to_string(), false)];
        let forms = PptxExtractor::math_forms(&formulas);

        let (text, found) = PptxExtractor::split_line_math("n", &forms, &formulas, false);
        assert_eq!(text, "n");
        assert!(found.is_empty());
    }

    /// Math inside a bulleted line becomes its own element, and the bullet keeps
    /// its words.
    #[test]
    fn test_build_internal_document_lifts_list_item_and_title_math() {
        use crate::types::internal::ElementKind;

        let content = "# Growth is $$g^{2}$$\n\n- Rate $r$ per year\n- Plain bullet\n";
        let formulas = vec![("g^{2}".to_string(), true), ("r".to_string(), false)];
        let mut budget = SecurityBudget::with_defaults();
        let doc = PptxExtractor::build_internal_document(&[(1, content.to_string())], 1, &formulas, false, &mut budget)
            .unwrap();

        let math: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Formula))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(math, vec!["g^{2}", "r"], "title and list-item math both emit");

        let items: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::ListItem { .. }))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(items, vec!["Rate per year", "Plain bullet"]);

        let headings: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Heading { .. }))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(headings, vec!["Growth is"], "the heading keeps its words");
    }

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
    fn test_archive_slide_contents_set_table_list_heading_and_paragraph_pages() {
        use crate::types::internal::ElementKind;

        let slide_contents = vec![
            (1, "# Titled slide\n\nIntroduction".to_string()),
            (
                2,
                concat!(
                    "This untitled slide has a paragraph long enough not to be inferred as a title. ",
                    "It deliberately contains more than one hundred characters in total."
                )
                .to_string(),
            ),
            (
                3,
                concat!(
                    "| Name | Value |\n",
                    "| --- | --- |\n",
                    "| answer | 42 |\n\n",
                    "- final item"
                )
                .to_string(),
            ),
        ];
        let mut budget = SecurityBudget::with_defaults();

        let document = PptxExtractor::build_internal_document(&slide_contents, 3, &[], false, &mut budget)
            .expect("internal PPTX document should build");

        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].page_number, 3);

        let list_item = document
            .elements
            .iter()
            .find(|element| matches!(element.kind, ElementKind::ListItem { .. }))
            .expect("list item should be present");
        assert_eq!(list_item.page, Some(3));

        let heading = document
            .elements
            .iter()
            .find(|element| matches!(element.kind, ElementKind::Heading { .. }))
            .expect("heading should be present");
        assert_eq!(heading.page, Some(1));

        let second_slide_paragraph = document
            .elements
            .iter()
            .find(|element| element.text.starts_with("This untitled slide"))
            .expect("second-slide paragraph should be present");
        assert_eq!(second_slide_paragraph.page, Some(2));
    }

    #[test]
    fn test_marker_like_slide_text_cannot_change_later_page_numbers() {
        let slide_contents = vec![
            (1, "First slide".to_string()),
            (
                2,
                "<!-- Slide number: 99 -->\n\n| Name | Value |\n| --- | --- |\n| answer | 42 |".to_string(),
            ),
        ];
        let mut budget = SecurityBudget::with_defaults();

        let document = PptxExtractor::build_internal_document(&slide_contents, 2, &[], false, &mut budget)
            .expect("marker-like user text should remain ordinary slide content");

        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].page_number, 2);
        assert!(document.elements.iter().any(|element| {
            matches!(element.kind, crate::types::internal::ElementKind::Paragraph)
                && element.text == "<!-- Slide number: 99 -->"
                && element.page == Some(2)
        }));
    }

    #[tokio::test]
    async fn test_untitled_slide_with_table_gets_archive_derived_page_numbers() {
        use crate::plugins::InternalDocumentExtractor;
        use crate::types::internal::ElementKind;

        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree>
        <p:sp><p:txBody><a:p><a:r><a:t>This untitled slide contains a deliberately long paragraph so the extractor cannot mistake it for a title while assigning page metadata.</a:t></a:r></a:p></p:txBody></p:sp>
        <p:graphicFrame>
            <a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
                    <a:tbl>
                        <a:tr>
                            <a:tc><a:txBody><a:p><a:r><a:t>Name</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc><a:txBody><a:p><a:r><a:t>Value</a:t></a:r></a:p></a:txBody></a:tc>
                        </a:tr>
                        <a:tr>
                            <a:tc><a:txBody><a:p><a:r><a:t>answer</a:t></a:r></a:p></a:txBody></a:tc>
                            <a:tc><a:txBody><a:p><a:r><a:t>42</a:t></a:r></a:p></a:txBody></a:tc>
                        </a:tr>
                    </a:tbl>
                </a:graphicData>
            </a:graphic>
        </p:graphicFrame>
    </p:spTree></p:cSld>
</p:sld>"#;
        let pptx = crate::extraction::pptx::tests::build_single_slide_pptx(slide_xml, None, &[]);
        let extractor = PptxExtractor::new();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };
        let mime = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

        let document = extractor
            .extract_content(&pptx, mime, &config)
            .await
            .expect("real PPTX extraction should succeed");

        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].page_number, 1);
        assert!(document.elements.iter().any(|element| {
            matches!(element.kind, ElementKind::Paragraph)
                && element.text.starts_with("This untitled slide")
                && element.page == Some(1)
        }));
        assert!(
            document
                .elements
                .iter()
                .any(|element| { matches!(element.kind, ElementKind::Slide { number: 1 }) && element.page == Some(1) })
        );
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
