//! JATS (Journal Article Tag Suite) document extractor.
//!
//! This extractor handles JATS XML documents, the standard format for scientific journal articles
//! used by PubMed Central and major academic publishers.
//!
//! It extracts:
//! - Rich metadata (title, subtitle, authors with affiliations, DOI, PII, keywords, dates)
//! - Article abstract (regular and graphical)
//! - Section hierarchy and content (intro, methods, results, discussion)
//! - Paragraphs and text content
//! - Tables with captions
//! - Figures with captions
//! - Citations and references
//! - Supplementary material information

mod elements;
mod metadata;
mod parser;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::text::utf8_validation;
use crate::types::Metadata;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::{ContributorRole, FormatMetadata, JatsMetadata};
use crate::types::uri::ExtractedUri;
use async_trait::async_trait;
use quick_xml::events::Event;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

use crate::utils::xml_utils::EntityReader;

use elements::extract_jats_all_in_one;
use parser::extract_citation_text as jats_extract_citation;
use parser::extract_fig_content as jats_extract_fig;
use parser::extract_formula_latex as jats_extract_formula;
use parser::extract_text_content as jats_extract_text;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const JATS_WARNING_SOURCE: &str = "jats";

/// Extract text and inline annotations from a JATS `<p>` element.
///
/// Recognizes:
/// - `<italic>` → italic
/// - `<bold>` → bold
/// - `<underline>` → underline
/// - `<sub>` → subscript
/// - `<sup>` → superscript
/// - `<ext-link>` → link (with xlink:href)
fn extract_para_with_annotations_jats(
    reader: &mut EntityReader<'_>,
    budget: &mut SecurityBudget,
) -> crate::Result<(String, Vec<crate::types::document_structure::TextAnnotation>)> {
    use crate::types::builder;

    let mut text = String::new();
    let mut annotations = Vec::new();
    let mut depth: u32 = 0;

    let mut inline_stack: Vec<(&'static str, u32, u32, Option<String>)> = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                depth += 1;

                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref());

                match tag.as_ref() {
                    "italic" => {
                        inline_stack.push(("italic", depth, text.len() as u32, None));
                    }
                    "bold" => {
                        inline_stack.push(("bold", depth, text.len() as u32, None));
                    }
                    "underline" => {
                        inline_stack.push(("underline", depth, text.len() as u32, None));
                    }
                    "sub" => {
                        inline_stack.push(("subscript", depth, text.len() as u32, None));
                    }
                    "sup" => {
                        inline_stack.push(("superscript", depth, text.len() as u32, None));
                    }
                    "ext-link" => {
                        let mut href = None;
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "xlink:href" || key.ends_with(":href") || key == "href" {
                                href = Some(val.to_string());
                            }
                        }
                        inline_stack.push(("link", depth, text.len() as u32, href));
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => {
                budget.leave();
                if depth == 0 {
                    break;
                }

                if let Some(&(kind, open_depth, start, ref href)) = inline_stack.last()
                    && open_depth == depth
                {
                    let end = text.len() as u32;
                    let actual_start = if (start as usize) < text.len() {
                        let span = &text[start as usize..end as usize];
                        let trimmed = span.trim_start();
                        end - trimmed.len() as u32
                    } else {
                        start
                    };
                    if end > actual_start {
                        let href_clone = href.clone();
                        let annotation = match kind {
                            "bold" => builder::bold(actual_start, end),
                            "italic" => builder::italic(actual_start, end),
                            "underline" => builder::underline(actual_start, end),
                            "subscript" => builder::subscript(actual_start, end),
                            "superscript" => builder::superscript(actual_start, end),
                            "link" => {
                                let url = href_clone.as_deref().unwrap_or("");
                                builder::link(actual_start, end, url, None)
                            }
                            // `inline_stack` is only ever pushed to with "italic", "bold",
                            // "underline", "subscript" (from "sub"), "superscript" (from
                            // "sup"), or "link" (from "ext-link") — every one of those six
                            // kinds is handled above, so no other value can appear.
                            _ => unreachable!("inline_stack only ever holds the six kinds handled above"),
                        };
                        annotations.push(annotation);
                    }
                    inline_stack.pop();
                }

                depth -= 1;
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                if !decoded.trim().is_empty() {
                    budget.check_entity(decoded.trim())?;
                    budget.account_text(decoded.trim().len())?;
                    if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                        text.push(' ');
                    }
                    text.push_str(decoded.trim());
                }
            }
            Ok(Event::CData(t)) => {
                let decoded = utf8_validation::from_utf8(t.as_ref()).unwrap_or("").to_string();
                if !decoded.trim().is_empty() {
                    budget.check_entity(decoded.trim())?;
                    budget.account_text(decoded.trim().len())?;
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(decoded.trim());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok((text.trim().to_string(), annotations))
}

/// Build an `InternalDocument` from JATS XML content.
fn build_jats_internal_document(content: &str, budget: &mut SecurityBudget) -> crate::Result<InternalDocument> {
    let mut reader = EntityReader::from_str(content);
    let mut builder = InternalDocumentBuilder::new("jats");

    let mut in_article_meta = false;
    let mut in_abstract = false;
    let mut in_body = false;
    let mut in_back = false;
    let mut in_ref_list = false;
    let mut in_table = false;
    let mut in_thead = false;
    let mut in_tbody = false;
    let mut in_row = false;
    let mut current_table: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut sec_depth: u32 = 0;
    let mut ref_list_opened = false;
    let mut back_list_opened = false;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref());

                match tag.as_ref() {
                    "article-meta" => {
                        in_article_meta = true;
                    }
                    "article-title" if in_article_meta && !in_abstract => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(1, &text, None, None);
                        }
                        continue;
                    }
                    "abstract" if in_article_meta => {
                        in_abstract = true;
                        builder.push_heading(2, "Abstract", None, None);
                    }
                    "sec" if in_abstract => {}
                    "title" if in_abstract => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(3, &text, None, None);
                        }
                        continue;
                    }
                    "p" if in_abstract => {
                        let (text, annotations) = extract_para_with_annotations_jats(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_paragraph(&text, annotations, None, None);
                        }
                        continue;
                    }
                    "body" => {
                        in_body = true;
                        sec_depth = 0;
                    }
                    "sec" if in_body => {
                        sec_depth += 1;
                    }
                    "title" if in_body && !in_article_meta && !in_ref_list => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            let level = (sec_depth + 1).min(6) as u8;
                            builder.push_heading(level, &text, None, None);
                        }
                        continue;
                    }
                    "p" if in_body => {
                        let (text, annotations) = extract_para_with_annotations_jats(&mut reader, budget)?;
                        if !text.is_empty() {
                            for ann in &annotations {
                                if let crate::types::document_structure::AnnotationKind::Link { url, .. } = &ann.kind
                                    && !url.is_empty()
                                {
                                    let label = text.get(ann.start as usize..ann.end as usize).map(|s| s.to_string());
                                    builder.push_uri(ExtractedUri::hyperlink(url, label));
                                }
                            }
                            builder.push_paragraph(&text, annotations, None, None);
                        }
                        continue;
                    }
                    "fig" if in_body => {
                        let (label, caption, href) = jats_extract_fig(&mut reader, budget)?;
                        let caption_full = match (&label, &caption) {
                            (Some(l), Some(c)) => format!("{}: {}", l, c),
                            (Some(l), None) => l.clone(),
                            (None, Some(c)) => c.clone(),
                            (None, None) => String::new(),
                        };
                        if !caption_full.is_empty() || href.is_some() {
                            let display = match (&href, caption_full.is_empty()) {
                                (Some(h), false) => format!("![{}]({})", caption_full, h),
                                (Some(h), true) => format!("![]({})", h),
                                (None, false) => caption_full.clone(),
                                (None, true) => String::new(),
                            };
                            if !display.is_empty() {
                                builder.push_paragraph(&display, Vec::new(), None, None);
                            }
                            if let Some(href) = &href {
                                let label_opt = if caption_full.is_empty() {
                                    None
                                } else {
                                    Some(caption_full.clone())
                                };
                                builder.push_uri(ExtractedUri::image(href, label_opt));
                            }
                        }
                        continue;
                    }
                    "disp-formula" | "inline-formula" if in_body => {
                        let latex = jats_extract_formula(&mut reader, budget)?;
                        if !latex.is_empty() {
                            builder.push_formula(&latex, None, None);
                        }
                        continue;
                    }
                    "back" => {
                        in_back = true;
                    }
                    "ack" if in_back => {}
                    "supplementary-material" if in_back => {
                        let _ = jats_extract_text(&mut reader, budget)?;
                        continue;
                    }
                    "title" if in_back && !in_ref_list => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(2, &text, None, None);
                        }
                        continue;
                    }
                    "p" if in_back && !in_ref_list => {
                        let (text, annotations) = extract_para_with_annotations_jats(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_paragraph(&text, annotations, None, None);
                        }
                        continue;
                    }
                    "term" if in_back && !in_ref_list => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_definition_term(&text, None);
                        }
                        continue;
                    }
                    "list-item" if in_back && !in_ref_list => {
                        let (text, annotations) = extract_para_with_annotations_jats(&mut reader, budget)?;
                        if !text.is_empty() {
                            if !back_list_opened {
                                builder.push_list(false);
                                back_list_opened = true;
                            }
                            builder.push_list_item(&text, false, annotations, None, None);
                        }
                        continue;
                    }
                    "table" => {
                        in_table = true;
                        current_table.clear();
                    }
                    "thead" if in_table => {
                        in_thead = true;
                    }
                    "tbody" if in_table => {
                        in_tbody = true;
                    }
                    "tr" if (in_thead || in_tbody) && in_table => {
                        in_row = true;
                        current_row.clear();
                    }
                    "td" | "th" if in_row => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        budget.add_cells(1)?;
                        current_row.push(text);
                        continue;
                    }
                    "ref-list" => {
                        in_ref_list = true;
                    }
                    "title" if in_ref_list => {
                        let text = jats_extract_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(2, &text, None, None);
                        }
                        continue;
                    }
                    "ref" if in_ref_list => {
                        let text = jats_extract_citation(&mut reader, budget)?;
                        if !text.is_empty() {
                            if !ref_list_opened {
                                builder.push_list(true);
                                ref_list_opened = true;
                            }
                            builder.push_list_item(&text, true, Vec::new(), None, None);
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref());

                match tag.as_ref() {
                    "article-meta" => {
                        in_article_meta = false;
                    }
                    "abstract" if in_abstract => {
                        in_abstract = false;
                    }
                    "body" => {
                        in_body = false;
                    }
                    "sec" if in_body && sec_depth > 0 => {
                        sec_depth -= 1;
                    }
                    "back" => {
                        in_back = false;
                    }
                    "list" if back_list_opened => {
                        builder.end_list();
                        back_list_opened = false;
                    }
                    "ref-list" => {
                        if ref_list_opened {
                            builder.end_list();
                            ref_list_opened = false;
                        }
                        in_ref_list = false;
                    }
                    "table" if in_table => {
                        if !current_table.is_empty() {
                            builder.push_table_from_cells(&current_table, None, None);
                            current_table.clear();
                        }
                        in_table = false;
                    }
                    "thead" if in_thead => {
                        in_thead = false;
                    }
                    "tbody" if in_tbody => {
                        in_tbody = false;
                    }
                    "tr" if in_row => {
                        if !current_row.is_empty() {
                            current_table.push(std::mem::take(&mut current_row));
                        }
                        in_row = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(builder.build())
}

/// JATS document extractor.
///
/// Supports JATS (Journal Article Tag Suite) XML documents in various versions,
/// handling both the full article structure and minimal JATS subsets.
#[cfg_attr(alef, alef(skip))]
pub struct JatsExtractor;

impl Default for JatsExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl JatsExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Plugin for JatsExtractor {
    fn name(&self) -> &str {
        "jats-extractor"
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
impl InternalDocumentExtractor for JatsExtractor {
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
        tracing::debug!(format = "jats", size_bytes = content.len(), "extraction starting");
        // Track the fallback: a non-UTF-8 article is decoded lossily and every byte the
        // decoder could not read is already U+FFFD before parsing starts (#171).
        let (jats_content, decoded_lossily) = match utf8_validation::from_utf8(content) {
            Ok(valid) => (valid.to_string(), false),
            Err(_) => (String::from_utf8_lossy(content).to_string(), true),
        };

        let (jats_metadata, _extracted_content, _title, _tables) = extract_jats_all_in_one(&jats_content)?;

        let mut metadata = Metadata::default();
        let mut subject_parts = Vec::new();

        if !jats_metadata.title.is_empty() {
            metadata.title = Some(jats_metadata.title.clone());
            metadata.subject = Some(jats_metadata.title.clone());
            subject_parts.push(format!("Title: {}", jats_metadata.title));
        }

        if let Some(subtitle) = &jats_metadata.subtitle {
            subject_parts.push(format!("Subtitle: {}", subtitle));
        }

        if !jats_metadata.authors.is_empty() {
            metadata.authors = Some(jats_metadata.authors.clone());
            subject_parts.push(format!("Authors: {}", jats_metadata.authors.join("; ")));
        }

        if !jats_metadata.affiliations.is_empty() {
            subject_parts.push(format!("Affiliations: {}", jats_metadata.affiliations.join("; ")));
        }

        if let Some(doi) = &jats_metadata.doi {
            subject_parts.push(format!("DOI: {}", doi));
        }

        if let Some(pii) = &jats_metadata.pii {
            subject_parts.push(format!("PII: {}", pii));
        }

        if !jats_metadata.keywords.is_empty() {
            metadata.keywords = Some(jats_metadata.keywords.clone());
            subject_parts.push(format!("Keywords: {}", jats_metadata.keywords.join("; ")));
        }

        if let Some(date) = &jats_metadata.publication_date {
            metadata.created_at = Some(date.clone());
            subject_parts.push(format!("Publication Date: {}", date));
        }

        if let Some(volume) = &jats_metadata.volume {
            subject_parts.push(format!("Volume: {}", volume));
        }

        if let Some(issue) = &jats_metadata.issue {
            subject_parts.push(format!("Issue: {}", issue));
        }

        if let Some(pages) = &jats_metadata.pages {
            subject_parts.push(format!("Pages: {}", pages));
        }

        if let Some(journal_title) = &jats_metadata.journal_title {
            subject_parts.push(format!("Journal: {}", journal_title));
        }

        if let Some(article_type) = &jats_metadata.article_type {
            subject_parts.push(format!("Article Type: {}", article_type));
        }

        if let Some(abstract_text) = &jats_metadata.abstract_text {
            subject_parts.push(format!("Abstract: {}", abstract_text));
        }

        if let Some(corresp_author) = &jats_metadata.corresponding_author {
            subject_parts.push(format!("Corresponding Author: {}", corresp_author));
        }

        let mut history_dates = std::collections::BTreeMap::new();
        if !jats_metadata.history_dates.is_empty() {
            for (date_type, date_val) in &jats_metadata.history_dates {
                subject_parts.push(format!(
                    "{}: {}",
                    date_type[..1].to_uppercase() + &date_type[1..],
                    date_val
                ));
                history_dates.insert(date_type.clone(), date_val.clone());
            }
        }

        let copyright = if let Some(copyright) = &jats_metadata.copyright_statement {
            subject_parts.push(format!("Copyright: {}", copyright));
            Some(copyright.clone())
        } else {
            None
        };

        let license = jats_metadata.license.clone();

        let contributor_roles: Vec<ContributorRole> = jats_metadata
            .contributor_roles
            .iter()
            .map(|(name, role)| ContributorRole {
                name: name.clone(),
                role: if role.is_empty() { None } else { Some(role.clone()) },
            })
            .collect();

        let jats_typed_metadata = JatsMetadata {
            copyright,
            license,
            history_dates,
            contributor_roles,
        };
        metadata.format = Some(FormatMetadata::Jats(jats_typed_metadata));

        if !subject_parts.is_empty() {
            metadata.subject = Some(subject_parts.join(" | "));
        }

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = build_jats_internal_document(&jats_content, &mut budget)?;
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;

        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                JATS_WARNING_SOURCE,
                "JATS source",
            );
        }

        if let Some(doi) = &jats_metadata.doi {
            doc.push_uri(ExtractedUri::citation(
                format!("https://doi.org/{}", doi),
                Some(format!("DOI: {}", doi)),
            ));
        }

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "jats",
            "extraction complete"
        );
        Ok(doc)
    }

    #[cfg(feature = "tokio-runtime")]
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, path, config),
            fields(
                extractor.name = self.name(),
            )
        )
    )]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let bytes = crate::core::io::read_file_async(path).await?;
        self.extract_content(&bytes, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-jats+xml", "text/jats"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elements::extract_jats_all_in_one;

    #[test]
    fn test_jats_extractor_plugin_interface() {
        let extractor = JatsExtractor::new();
        assert_eq!(extractor.name(), "jats-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_jats_extractor_supported_mime_types() {
        let extractor = JatsExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 2);
        assert!(mime_types.contains(&"application/x-jats+xml"));
        assert!(mime_types.contains(&"text/jats"));
    }

    #[test]
    fn test_jats_extractor_priority() {
        let extractor = JatsExtractor::new();
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_parse_simple_jats_article() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Test Article Title</article-title>
    </article-meta>
  </front>
  <body>
    <p>Test content paragraph.</p>
  </body>
</article>"#;

        let (metadata, content, title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert_eq!(title, "Test Article Title");
        assert_eq!(metadata.title, "Test Article Title");
        assert!(content.contains("Test Article Title"));
        assert!(content.contains("Test content"));
    }

    #[test]
    fn test_extract_jats_title() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Effects of Caffeine on Human Health</article-title>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.title, "Effects of Caffeine on Human Health");
    }

    #[test]
    fn test_extract_jats_subtitle() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Main Title</article-title>
      <subtitle>A Systematic Review</subtitle>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.title, "Main Title");
        assert_eq!(metadata.subtitle, Some("A Systematic Review".to_string()));
    }

    #[test]
    fn test_extract_jats_authors() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <contrib-group>
        <contrib contrib-type="author">
          <name>
            <surname>Smith</surname>
            <given-names>John A.</given-names>
          </name>
        </contrib>
        <contrib contrib-type="author">
          <name>
            <surname>Johnson</surname>
            <given-names>Jane B.</given-names>
          </name>
        </contrib>
      </contrib-group>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.authors.len(), 2);
        assert!(metadata.authors[0].contains("Smith"));
        assert!(metadata.authors[1].contains("Johnson"));
    }

    #[test]
    fn test_extract_jats_affiliations() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <aff id="aff1">Department of Medicine, Harvard University, Cambridge, MA</aff>
      <aff id="aff2">Center for Health Research, Boston Medical Center, Boston, MA</aff>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.affiliations.len(), 2);
        assert!(metadata.affiliations[0].contains("Harvard"));
        assert!(metadata.affiliations[1].contains("Boston"));
    }

    #[test]
    fn test_extract_jats_doi_and_pii() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-id pub-id-type="doi">10.1371/journal.pmed.0020124</article-id>
      <article-id pub-id-type="pii">05-PLME-RA-0071R2</article-id>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.doi, Some("10.1371/journal.pmed.0020124".to_string()));
        assert_eq!(metadata.pii, Some("05-PLME-RA-0071R2".to_string()));
    }

    #[test]
    fn test_extract_jats_keywords() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <kwd-group>
        <kwd>caffeine</kwd>
        <kwd>meta-analysis</kwd>
        <kwd>systematic review</kwd>
      </kwd-group>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.keywords.len(), 3);
        assert!(metadata.keywords.contains(&"caffeine".to_string()));
        assert!(metadata.keywords.contains(&"meta-analysis".to_string()));
    }

    #[test]
    fn test_extract_jats_publication_info() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <pub-date pub-type="epub">
        <day>18</day>
        <month>04</month>
        <year>2005</year>
      </pub-date>
      <volume>2</volume>
      <issue>4</issue>
      <fpage>e124</fpage>
      <lpage>e132</lpage>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert!(metadata.publication_date.is_some());
        assert_eq!(metadata.volume, Some("2".to_string()));
        assert_eq!(metadata.issue, Some("4".to_string()));
        assert!(metadata.pages.is_some());
    }

    #[test]
    fn test_extract_jats_abstract() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <abstract>
        <sec>
          <title>Background</title>
          <p>This is the background information of the study.</p>
        </sec>
        <sec>
          <title>Methods</title>
          <p>We used quantitative analysis to evaluate the hypothesis.</p>
        </sec>
      </abstract>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert!(metadata.abstract_text.is_some());
        let abstract_text = metadata.abstract_text.expect("abstract_text should be present");
        assert!(abstract_text.contains("background"));
        assert!(abstract_text.contains("quantitative"));
    }

    #[test]
    fn test_extract_jats_tables_basic() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <body>
    <table-wrap id="tbl1">
      <table>
        <thead>
          <tr>
            <th>Study</th>
            <th>Year</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Study A</td>
            <td>2003</td>
          </tr>
          <tr>
            <td>Study B</td>
            <td>2004</td>
          </tr>
        </tbody>
      </table>
    </table-wrap>
  </body>
</article>"#;

        let (_metadata, _content, _title, tables) = extract_jats_all_in_one(jats).expect("Table extraction failed");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].cells.len(), 3);
    }

    #[test]
    fn test_extract_jats_corresponding_author() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <author-notes>
        <corresp id="cor1">To whom correspondence should be addressed. E-mail: rwilliams@yale.edu</corresp>
      </author-notes>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert!(metadata.corresponding_author.is_some());
        let corresp = metadata
            .corresponding_author
            .expect("corresponding_author should be present");
        assert!(corresp.contains("rwilliams"));
    }

    #[test]
    fn test_extract_jats_section_hierarchy() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Article Title</article-title>
    </article-meta>
  </front>
  <body>
    <sec id="s1">
      <title>Introduction</title>
      <p>Intro content.</p>
    </sec>
    <sec id="s2">
      <title>Methods</title>
      <p>Methods content.</p>
    </sec>
    <sec id="s3">
      <title>Results</title>
      <p>Results content.</p>
    </sec>
  </body>
</article>"#;

        let (_metadata, content, _title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert!(content.contains("Introduction"));
        assert!(content.contains("Methods"));
        assert!(content.contains("Results"));
    }

    #[test]
    fn test_jats_extractor_full_metadata_extraction() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Sample Article</article-title>
      <contrib-group>
        <contrib contrib-type="author">
          <name>
            <surname>Smith</surname>
            <given-names>John</given-names>
          </name>
        </contrib>
      </contrib-group>
      <article-id pub-id-type="doi">10.1234/test</article-id>
      <kwd-group>
        <kwd>test</kwd>
      </kwd-group>
      <abstract>
        <p>Test abstract.</p>
      </abstract>
    </article-meta>
  </front>
  <body>
    <p>Sample content.</p>
  </body>
</article>"#;

        let (metadata_extracted, _content, _title, _tables) =
            extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata_extracted.title, "Sample Article");
        assert_eq!(metadata_extracted.authors.len(), 1);
        assert_eq!(metadata_extracted.doi, Some("10.1234/test".to_string()));
        assert_eq!(metadata_extracted.keywords.len(), 1);
        assert!(metadata_extracted.abstract_text.is_some());
    }

    #[test]
    fn test_jats_extractor_empty_article() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
    </article-meta>
  </front>
  <body>
  </body>
</article>"#;

        let (metadata, content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert!(metadata.title.is_empty());
        assert!(content.is_empty() || content.trim().is_empty());
    }

    #[test]
    fn test_extract_jats_journal_title() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Test Article</article-title>
      <journal-title>Nature Medicine</journal-title>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.journal_title, Some("Nature Medicine".to_string()));
    }

    #[test]
    fn test_extract_jats_article_type() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article article-type="research-article">
  <front>
    <article-meta>
      <article-title>Test Article</article-title>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");
        assert_eq!(metadata.article_type, Some("research-article".to_string()));
    }

    #[test]
    fn test_extract_all_13_metadata_fields() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article article-type="research-article">
  <front>
    <article-meta>
      <article-title>Full Metadata Test</article-title>
      <subtitle>A Complete Example</subtitle>
      <contrib-group>
        <contrib contrib-type="author">
          <name>
            <surname>Author</surname>
            <given-names>First</given-names>
          </name>
        </contrib>
      </contrib-group>
      <aff>Department of Testing, Test University</aff>
      <article-id pub-id-type="doi">10.1234/full-test</article-id>
      <article-id pub-id-type="pii">TEST-001</article-id>
      <kwd-group>
        <kwd>testing</kwd>
        <kwd>metadata</kwd>
      </kwd-group>
      <pub-date pub-type="epub">
        <year>2024</year>
      </pub-date>
      <volume>5</volume>
      <issue>3</issue>
      <fpage>100</fpage>
      <lpage>110</lpage>
      <journal-title>Test Journal</journal-title>
      <abstract>
        <p>This is a test abstract for all metadata fields.</p>
      </abstract>
      <author-notes>
        <corresp>Correspondence: test@example.com</corresp>
      </author-notes>
    </article-meta>
  </front>
  <body>
    <p>Test content.</p>
  </body>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Metadata extraction failed");

        assert_eq!(metadata.title, "Full Metadata Test");
        assert_eq!(metadata.subtitle, Some("A Complete Example".to_string()));
        assert_eq!(metadata.authors.len(), 1);
        assert!(metadata.authors[0].contains("Author"));
        assert_eq!(metadata.affiliations.len(), 1);
        assert!(metadata.affiliations[0].contains("Testing"));
        assert_eq!(metadata.doi, Some("10.1234/full-test".to_string()));
        assert_eq!(metadata.pii, Some("TEST-001".to_string()));
        assert_eq!(metadata.keywords.len(), 2);
        assert!(metadata.publication_date.is_some());
        assert_eq!(metadata.volume, Some("5".to_string()));
        assert_eq!(metadata.issue, Some("3".to_string()));
        assert!(metadata.pages.is_some());
        assert_eq!(metadata.journal_title, Some("Test Journal".to_string()));
        assert_eq!(metadata.article_type, Some("research-article".to_string()));
        assert!(metadata.abstract_text.is_some());
        assert!(metadata.corresponding_author.is_some());
    }

    #[test]
    fn test_extract_jats_history_dates() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Test Article</article-title>
      <history>
        <date date-type="received">
          <day>15</day>
          <month>01</month>
          <year>2024</year>
        </date>
        <date date-type="accepted">
          <day>20</day>
          <month>03</month>
          <year>2024</year>
        </date>
      </history>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert_eq!(metadata.history_dates.len(), 2);
        assert_eq!(metadata.history_dates[0].0, "received");
        assert!(metadata.history_dates[0].1.contains("2024"));
        assert_eq!(metadata.history_dates[1].0, "accepted");
    }

    #[test]
    fn test_extract_jats_permissions() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Test Article</article-title>
      <permissions>
        <copyright-statement>Copyright 2024 The Authors</copyright-statement>
        <license>
          <license-p>This is an open-access article distributed under the CC BY 4.0 license.</license-p>
        </license>
      </permissions>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert_eq!(
            metadata.copyright_statement,
            Some("Copyright 2024 The Authors".to_string())
        );
        assert!(metadata.license.is_some());
        assert!(
            metadata
                .license
                .expect("license should be present")
                .contains("CC BY 4.0")
        );
    }

    #[test]
    fn test_extract_jats_inline_formula() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Formula Test</article-title>
    </article-meta>
  </front>
  <body>
    <p>The equation <inline-formula>E = mc^2</inline-formula> is famous.</p>
  </body>
</article>"#;

        let (_metadata, content, _title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert!(content.contains("E = mc^2"), "expected inline formula, got: {content}");
    }

    #[test]
    fn test_extract_jats_contributor_roles() {
        let jats = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <front>
    <article-meta>
      <article-title>Test Article</article-title>
      <contrib-group>
        <contrib contrib-type="author">
          <name>
            <surname>Smith</surname>
            <given-names>John</given-names>
          </name>
        </contrib>
        <contrib contrib-type="editor">
          <name>
            <surname>Jones</surname>
            <given-names>Mary</given-names>
          </name>
        </contrib>
      </contrib-group>
    </article-meta>
  </front>
</article>"#;

        let (metadata, _content, _title, _tables) = extract_jats_all_in_one(jats).expect("Parse failed");
        assert_eq!(metadata.contributor_roles.len(), 2);

        let author_role = metadata
            .contributor_roles
            .iter()
            .find(|(_, role)| role == "author")
            .expect("expected author role");
        assert!(author_role.0.contains("Smith"));

        let editor_role = metadata
            .contributor_roles
            .iter()
            .find(|(_, role)| role == "editor")
            .expect("expected editor role");
        assert!(editor_role.0.contains("Jones"));
    }
}
