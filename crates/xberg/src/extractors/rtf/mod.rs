//! RTF (Rich Text Format) extractor.
//!
//! Supports: Rich Text Format (.rtf)
//!
//! This native Rust extractor provides text extraction from RTF documents with:
//! - Character encoding support (Windows-1252 for 0x80-0x9F range)
//! - Common RTF control words (paragraph breaks, tabs, bullets, quotes, dashes)
//! - Unicode escape sequences
//! - Image metadata extraction
//! - Whitespace normalization

mod encoding;
mod formatting;
mod images;
mod metadata;
mod parser;
mod tables;

pub use formatting::normalize_whitespace;
pub(crate) use metadata::extract_rtf_metadata;
pub(crate) use parser::{extract_rtf_formatting, extract_text_from_rtf, spans_to_annotations};

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use crate::types::uri::ExtractedUri;
use async_trait::async_trait;
use bytes::Bytes;
use std::borrow::Cow;

/// Native Rust RTF extractor.
///
/// Extracts text content, metadata, and structure from RTF documents
#[cfg_attr(alef, alef(skip))]
pub struct RtfExtractor;

impl RtfExtractor {
    /// Create a new RTF extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for RtfExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl RtfExtractor {
    /// Build an `InternalDocument` from RTF content.
    ///
    /// Extracts paragraphs (split on double-newlines), tables, formatting
    /// annotations, and header/footer content with appropriate content layers.
    fn build_internal_document(rtf_content: &str, plain: bool) -> InternalDocument {
        use crate::types::document_structure::ContentLayer;

        let (extracted_text, tables, rtf_images, para_metas, mut formatting) =
            extract_text_from_rtf(rtf_content, plain);
        let legacy_formatting = extract_rtf_formatting(rtf_content);
        formatting.header_text = legacy_formatting.header_text;
        formatting.footer_text = legacy_formatting.footer_text;

        let mut builder = InternalDocumentBuilder::new("rtf");

        for (_start, _end, url) in &formatting.hyperlinks {
            if !url.is_empty() {
                let label = extracted_text.get(*_start..*_end).map(|s| s.to_string());
                builder.push_uri(ExtractedUri::hyperlink(url, label));
            }
        }

        let mut table_idx = 0;
        let mut meta_idx = 0;
        let mut byte_offset = 0usize;
        let mut in_table_rows = false;

        let mut in_list = false;
        let mut list_id: Option<u16> = None;
        let mut list_depth: u8 = 0;

        for paragraph in extracted_text.split("\n\n") {
            let trimmed = paragraph.trim();
            if trimmed.is_empty() {
                byte_offset += paragraph.len() + 2;
                meta_idx += 1;
                continue;
            }

            let meta = para_metas.get(meta_idx).cloned().unwrap_or_default();
            meta_idx += 1;

            if meta.is_table {
                if in_list {
                    for _ in 0..=list_depth {
                        builder.end_list();
                    }
                    in_list = false;
                    list_id = None;
                    list_depth = 0;
                }
                in_table_rows = true;
                byte_offset += paragraph.len() + 2;
                continue;
            }

            if in_table_rows {
                in_table_rows = false;
                if table_idx < tables.len() {
                    builder.push_table_from_cells(&tables[table_idx].cells, None, None);
                    table_idx += 1;
                }
            }

            let lines: Vec<&str> = trimmed.lines().collect();
            let is_table_like = lines.len() >= 2 && lines.iter().all(|l| l.contains('|'));
            if is_table_like && table_idx < tables.len() {
                if in_list {
                    for _ in 0..=list_depth {
                        builder.end_list();
                    }
                    in_list = false;
                    list_id = None;
                    list_depth = 0;
                }
                builder.push_table_from_cells(&tables[table_idx].cells, None, None);
                table_idx += 1;
                byte_offset += paragraph.len() + 2;
                continue;
            }

            let trim_offset = byte_offset + (paragraph.len() - paragraph.trim_start().len());
            let annotations = spans_to_annotations(trim_offset, trim_offset + trimmed.len(), &formatting);

            if meta.heading_level > 0 && meta.heading_level <= 6 {
                if in_list {
                    for _ in 0..=list_depth {
                        builder.end_list();
                    }
                    in_list = false;
                    list_id = None;
                    list_depth = 0;
                }
                builder.push_heading(meta.heading_level, trimmed, None, None);
            } else if let Some(level) = meta.list_level {
                let new_list_id = meta.list_id;
                let ordered = meta.ordered;

                if !in_list || list_id != new_list_id {
                    if in_list {
                        for _ in 0..=list_depth {
                            builder.end_list();
                        }
                    }
                    builder.push_list(ordered);
                    for _ in 0..level {
                        builder.push_list(ordered);
                    }
                    in_list = true;
                    list_id = new_list_id;
                    list_depth = level;
                } else if level > list_depth {
                    for _ in list_depth..level {
                        builder.push_list(ordered);
                    }
                    list_depth = level;
                } else if level < list_depth {
                    for _ in level..list_depth {
                        builder.end_list();
                    }
                    list_depth = level;
                }

                builder.push_list_item(trimmed, ordered, annotations, None, None);
            } else {
                if in_list {
                    for _ in 0..=list_depth {
                        builder.end_list();
                    }
                    in_list = false;
                    list_id = None;
                    list_depth = 0;
                }
                builder.push_paragraph(trimmed, annotations, None, None);
            }
            byte_offset += paragraph.len() + 2;
        }

        if in_list {
            for _ in 0..=list_depth {
                builder.end_list();
            }
        }

        if in_table_rows && table_idx < tables.len() {
            builder.push_table_from_cells(&tables[table_idx].cells, None, None);
            table_idx += 1;
        }

        while table_idx < tables.len() {
            builder.push_table_from_cells(&tables[table_idx].cells, None, None);
            table_idx += 1;
        }

        for (i, rtf_img) in rtf_images.into_iter().enumerate() {
            let (image_kind, kind_confidence) =
                crate::extraction::image_kind::classify(&rtf_img.data, rtf_img.format, None, None, None, None, false);

            let image = ExtractedImage {
                data: Bytes::from(rtf_img.data),
                format: Cow::Borrowed(rtf_img.format),
                image_index: i as u32,
                page_number: None,
                width: None,
                height: None,
                colorspace: None,
                bits_per_component: None,
                is_mask: false,
                description: Some("image".to_string()),
                ocr_result: None,
                bounding_box: None,
                source_path: None,
                image_kind: Some(image_kind),
                kind_confidence: Some(kind_confidence),
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            };
            builder.push_image(None, image, None, None);
        }

        if let Some(ref header) = formatting.header_text {
            let idx = builder.push_paragraph(header, vec![], None, None);
            builder.set_layer(idx, ContentLayer::Header);
        }
        if let Some(ref footer) = formatting.footer_text {
            let idx = builder.push_paragraph(footer, vec![], None, None);
            builder.set_layer(idx, ContentLayer::Footer);
        }

        builder.build()
    }
}

impl Plugin for RtfExtractor {
    fn name(&self) -> &str {
        "rtf-extractor"
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
        "Extracts content from RTF (Rich Text Format) files with native Rust parsing"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for RtfExtractor {
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
        tracing::debug!(format = "rtf", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;
        let rtf_content = String::from_utf8_lossy(content);
        let plain = true;

        let (extracted_text, _tables, _images, _metas, _formatting_data) = extract_text_from_rtf(&rtf_content, plain);
        let mut metadata_map = extract_rtf_metadata(&rtf_content, &extracted_text);

        let title = metadata_map
            .remove(&Cow::Borrowed("title"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let subject = metadata_map
            .remove(&Cow::Borrowed("subject"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let authors = metadata_map.remove(&Cow::Borrowed("authors")).and_then(|v| {
            v.as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        });
        let created_by = metadata_map
            .remove(&Cow::Borrowed("created_by"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let modified_by = metadata_map
            .remove(&Cow::Borrowed("modified_by"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let created_at = metadata_map
            .remove(&Cow::Borrowed("created_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let modified_at = metadata_map
            .remove(&Cow::Borrowed("modified_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let mut doc = Self::build_internal_document(&rtf_content, plain);

        if let Some(ref filter) = config.content_filter {
            use crate::types::document_structure::ContentLayer;
            doc.elements.retain(|elem| match elem.layer {
                ContentLayer::Header => filter.include_headers,
                ContentLayer::Footer => filter.include_footers,
                _ => true,
            });
        }

        doc.mime_type = mime_type.to_string();
        doc.metadata = Metadata {
            title,
            subject,
            authors,
            created_by,
            modified_by,
            created_at,
            modified_at,
            additional: metadata_map,
            ..Default::default()
        };

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "rtf",
            "extraction complete"
        );
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/rtf", "text/rtf"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rtf_extractor_plugin_interface() {
        let extractor = RtfExtractor::new();
        assert_eq!(extractor.name(), "rtf-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert!(extractor.supported_mime_types().contains(&"application/rtf"));
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_simple_rtf_extraction() {
        let _extractor = RtfExtractor;
        let rtf_content = r#"{\rtf1 Hello World}"#;
        let (extracted, _, _, _, _) = extract_text_from_rtf(rtf_content, false);
        assert!(extracted.contains("Hello") || extracted.contains("World"));
    }

    #[test]
    fn test_plain_text_no_image_markdown() {
        let rtf_content = r#"{\rtf1 Before image {\pict\jpegblip\picw100\pich100 ffd8ffe0} After image}"#;
        let (plain, _, _, _, _) = extract_text_from_rtf(rtf_content, true);
        assert!(!plain.contains("!["), "Plain text should not contain image markdown");
        assert!(
            !plain.contains("]("),
            "Plain text should not contain image markdown links"
        );

        let (md, _, _, _, _) = extract_text_from_rtf(rtf_content, false);
        assert!(md.contains("![image]"), "Markdown output should contain image markers");
    }

    #[test]
    fn test_plain_text_table_uses_pipes() {
        let rtf_content = r#"{\rtf1 {\trowd Cell1\cell Cell2\cell\row}}"#;
        let (plain, tables, _, _, _) = extract_text_from_rtf(rtf_content, true);
        assert!(
            plain.contains("[TABLE_ROW]"),
            "Plain text should contain table row placeholder, got: {}",
            plain
        );
        assert!(!tables.is_empty(), "Tables should still be extracted");
    }

    #[test]
    fn test_rtf_bold_formatting_extraction() {
        let rtf_content = r#"{\rtf1 Normal {\b Bold text} more normal}"#;
        let formatting = extract_rtf_formatting(rtf_content);
        let has_bold = formatting.spans.iter().any(|s| s.bold);
        assert!(has_bold, "Should detect bold formatting in RTF");
    }

    #[test]
    fn test_rtf_italic_formatting_extraction() {
        let rtf_content = r#"{\rtf1 Normal {\i Italic text} more normal}"#;
        let formatting = extract_rtf_formatting(rtf_content);
        let has_italic = formatting.spans.iter().any(|s| s.italic);
        assert!(has_italic, "Should detect italic formatting in RTF");
    }

    #[test]
    fn test_rtf_underline_formatting_extraction() {
        let rtf_content = r#"{\rtf1 Normal {\ul Underlined text} more normal}"#;
        let formatting = extract_rtf_formatting(rtf_content);
        let has_underline = formatting.spans.iter().any(|s| s.underline);
        assert!(has_underline, "Should detect underline formatting in RTF");
    }

    #[test]
    fn test_rtf_color_table_extraction() {
        let rtf_content = r#"{\rtf1{\colortbl;\red255\green0\blue0;\red0\green255\blue0;}Normal {\cf1 Red text} more}"#;
        let formatting = extract_rtf_formatting(rtf_content);
        assert!(
            formatting.color_table.len() >= 2,
            "Should parse at least 2 color entries"
        );
        assert_eq!(formatting.color_table[1], "#ff0000", "First color should be red");
    }

    #[test]
    fn test_rtf_header_footer_extraction() {
        let rtf_content = r#"{\rtf1{\header My Header Text}{\footer My Footer Text}Body text}"#;
        let formatting = extract_rtf_formatting(rtf_content);
        assert!(formatting.header_text.is_some(), "Should extract header text");
        assert!(formatting.footer_text.is_some(), "Should extract footer text");
        assert!(
            formatting.header_text.as_deref().unwrap_or("").contains("Header"),
            "Header text should contain 'Header'"
        );
        assert!(
            formatting.footer_text.as_deref().unwrap_or("").contains("Footer"),
            "Footer text should contain 'Footer'"
        );
    }

    #[tokio::test]
    async fn test_rtf_document_structure_with_annotations() {
        let rtf_content = r#"{\rtf1 Normal text\par {\b Bold paragraph}\par More normal text}"#;
        let extractor = RtfExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(rtf_content.as_bytes(), "application/rtf", &config)
            .await
            .expect("RTF extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(result.document.is_some(), "Should produce document structure");
        let doc = result.document.unwrap();
        assert!(!doc.nodes.is_empty(), "Document structure should have nodes");
    }

    #[test]
    fn test_bookmark_hyperlink_boundaries() {
        let rtf_content = r#"{\rtf1\ansi
\pard
{\*\bkmkstart bookmark_1}Bookmark_1{\*\bkmkend bookmark_1}
\par
\pard
{\field{\*\fldinst { HYPERLINK  \\l "bookmark_1" }}{\fldrslt{click me}}}
\par
}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        assert!(!formatting.hyperlinks.is_empty(), "Should have hyperlinks");
        let (start, end, ref url) = formatting.hyperlinks[0];
        let link_text = &text[start..end];
        assert_eq!(
            link_text, "click me",
            "Hyperlink should cover 'click me', got: {:?}",
            link_text
        );
        assert_eq!(url, "#bookmark_1");
    }

    #[test]
    fn test_rtf_ansicpg1251_hex_escape_extraction() {
        let rtf_content =
            r#"{\rtf1\ansi\ansicpg1251\deff0{\fonttbl{\f0\fnil\fcharset204 Arial;}}\f0 \'cf\'f0\'e8\'e2\'e5\'f2}"#;
        let (text, _, _, _, _) = extract_text_from_rtf(rtf_content, true);

        assert!(text.contains("Привет"), "expected readable Cyrillic, got: {text:?}");
        assert!(
            !text.contains("Ïðèâåò"),
            "should not decode CP1251 bytes as Windows-1252"
        );
    }

    #[test]
    fn test_rtf_ansicpg932_multibyte_run() {
        let rtf_content =
            r#"{\rtf1\ansi\ansicpg932\deff0{\fonttbl{\f0\fnil\fcharset128 MS Mincho;}}\f0 \'93\'fa\'96\'7b}"#;
        let (text, _, _, _, _) = extract_text_from_rtf(rtf_content, true);

        assert!(
            text.contains("日本"),
            "adjacent escapes should decode as one Shift-JIS run, got: {text:?}"
        );
        assert!(!text.contains('\u{FFFD}'), "run must not decode byte-by-byte");
    }

    #[test]
    fn test_rtf_hex_escape_run_split_by_newline() {
        let rtf_content = "{\\rtf1\\ansi\\ansicpg932 \\'93\r\n\\'fa}";
        let (text, _, _, _, _) = extract_text_from_rtf(rtf_content, true);

        assert!(
            text.contains('日'),
            "a line wrap between the bytes of one character must not split the run, got: {text:?}"
        );
        assert!(!text.contains('\u{FFFD}'), "wrapped run must not decode as U+FFFD");
    }

    #[test]
    fn test_rtf_ansicpg_codepage_table() {
        let cases = [
            (1251, r"\'cf\'f0\'e8\'e2\'e5\'f2", "Привет"),
            (932, r"\'93\'fa\'96\'7b", "日本"),
            (936, r"\'c4\'e3\'ba\'c3", "你好"),
            (874, r"\'a1", "ก"),
            (866, r"\'80\'81", "АБ"),
            (20866, r"\'e1\'e2", "АБ"),
            // Unknown codepage falls back to Windows-1252.
            (1717, r"\'e9", "é"),
        ];
        for (codepage, escapes, expected) in cases {
            let rtf_content = format!(r"{{\rtf1\ansi\ansicpg{codepage} {escapes}}}");
            let (text, _, _, _, _) = extract_text_from_rtf(&rtf_content, true);
            assert!(
                text.contains(expected),
                "codepage {codepage}: expected {expected:?}, got: {text:?}"
            );
        }
    }

    #[test]
    fn test_rtf_multibyte_formatting_span_alignment() {
        let rtf_content = r#"{\rtf1\ansi\ansicpg932 before {\b \'93\'fa\'96\'7b} after}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        let bold_spans: Vec<_> = formatting.spans.iter().filter(|s| s.bold).collect();
        assert!(!bold_spans.is_empty(), "Should have bold spans");
        let span = &bold_spans[0];
        let bold_text = &text[span.start..span.end];
        assert_eq!(
            bold_text, "日本",
            "Bold span should exactly cover the decoded multi-byte run, got: {bold_text:?}"
        );
    }

    #[test]
    fn test_bold_italic_span_alignment() {
        let rtf_content = r#"{\rtf1 Normal {\b bold text} more normal}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        let bold_spans: Vec<_> = formatting.spans.iter().filter(|s| s.bold).collect();
        assert!(!bold_spans.is_empty(), "Should have bold spans");
        let span = &bold_spans[0];
        let bold_text = &text[span.start..span.end];
        assert_eq!(
            bold_text, "bold text",
            "Bold span should exactly cover 'bold text', got: {:?}",
            bold_text
        );
    }

    #[test]
    fn test_italic_span_alignment() {
        let rtf_content = r#"{\rtf1 Normal {\i italic text} more normal}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        let italic_spans: Vec<_> = formatting.spans.iter().filter(|s| s.italic).collect();
        assert!(!italic_spans.is_empty(), "Should have italic spans");
        let span = &italic_spans[0];
        let italic_text = &text[span.start..span.end];
        assert_eq!(
            italic_text, "italic text",
            "Italic span should exactly cover 'italic text', got: {:?}",
            italic_text
        );
    }

    #[test]
    fn test_bold_and_italic_span_alignment() {
        let rtf_content = r#"{\rtf1 {\b bold }{\b\i and italics} end}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        let bold_spans: Vec<_> = formatting.spans.iter().filter(|s| s.bold).collect();
        assert!(!bold_spans.is_empty(), "Should have bold spans");
        let full_bold: String = bold_spans
            .iter()
            .map(|s| &text[s.start..s.end])
            .collect::<Vec<_>>()
            .join("");
        assert!(
            full_bold.contains("bold"),
            "Bold text should include 'bold', got: {:?}",
            full_bold
        );
        let bi_spans: Vec<_> = formatting.spans.iter().filter(|s| s.bold && s.italic).collect();
        assert!(!bi_spans.is_empty(), "Should have bold+italic spans");
        let bi_text = &text[bi_spans[0].start..bi_spans[0].end];
        assert_eq!(
            bi_text, "and italics",
            "Bold+italic span should cover 'and italics', got: {:?}",
            bi_text
        );
    }

    #[test]
    fn test_formatting_with_paragraph_breaks() {
        let rtf_content = r#"{\rtf1 Normal text\par {\b bold}\par {\i italics}\par }"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, true);
        let bold_spans: Vec<_> = formatting.spans.iter().filter(|s| s.bold).collect();
        if !bold_spans.is_empty() {
            let span = &bold_spans[0];
            assert!(
                span.end <= text.len(),
                "Bold span end {} exceeds text length {}",
                span.end,
                text.len()
            );
            let bold_text = &text[span.start..span.end];
            assert_eq!(bold_text, "bold", "Bold span should cover 'bold', got: {:?}", bold_text);
        }
        let italic_spans: Vec<_> = formatting.spans.iter().filter(|s| s.italic).collect();
        if !italic_spans.is_empty() {
            let span = &italic_spans[0];
            assert!(
                span.end <= text.len(),
                "Italic span end {} exceeds text length {}",
                span.end,
                text.len()
            );
            let italic_text = &text[span.start..span.end];
            assert_eq!(
                italic_text, "italics",
                "Italic span should cover 'italics', got: {:?}",
                italic_text
            );
        }
    }

    #[test]
    fn test_bookmark_hyperlink_with_formatting() {
        let rtf_content = r#"{\rtf1\ansi
\pard
{\*\bkmkstart bookmark_1}Bookmark_1{\*\bkmkend bookmark_1}
\par
\pard
{\field{\*\fldinst { HYPERLINK  \\l "bookmark_1" }}{\fldrslt {\cf1\ul click me}}}
\par
}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        assert!(!formatting.hyperlinks.is_empty(), "Should have hyperlinks");
        let (start, end, ref url) = formatting.hyperlinks[0];
        let link_text = &text[start..end];
        assert_eq!(
            link_text, "click me",
            "Hyperlink should cover 'click me', got: {:?}",
            link_text
        );
        assert_eq!(url, "#bookmark_1");
    }

    #[test]
    fn test_bookmark_hyperlink_with_extra_spaces() {
        let rtf_content = r#"{\rtf1\ansi
\pard
{\*\bkmkstart bookmark_1}Bookmark_1{\*\bkmkend bookmark_1}
\par
\pard
Some  text  before {\field{\*\fldinst { HYPERLINK  \\l "bookmark_1" }}{\fldrslt {\cf1\ul click me}}} after
\par
}"#;
        let (text, _, _, _, formatting) = extract_text_from_rtf(rtf_content, false);
        assert!(!formatting.hyperlinks.is_empty(), "Should have hyperlinks");
        let (start, end, ref url) = formatting.hyperlinks[0];
        let link_text = &text[start..end];
        assert_eq!(
            link_text, "click me",
            "Hyperlink should cover 'click me', got: {:?}",
            link_text
        );
        assert_eq!(url, "#bookmark_1");
    }

    #[test]
    fn test_listtext_ordered_detection() {
        let rtf_content = r#"{\rtf1\ansi
\pard\ilvl0\ls1{\listtext 1.\tab}First item\par
\pard\ilvl0\ls1{\listtext 2.\tab}Second item\par
}"#;
        let (_text, _, _, metas, _) = extract_text_from_rtf(rtf_content, true);
        let ordered_items: Vec<_> = metas.iter().filter(|m| m.ordered && m.list_level.is_some()).collect();
        assert!(
            !ordered_items.is_empty(),
            "Should have ordered list items, metas: {:?}",
            metas
        );
    }

    #[test]
    fn test_bookmark_hyperlink_end_to_end() {
        let rtf_content = r#"{\rtf1\ansi
\pard
{\*\bkmkstart bookmark_1}Bookmark_1{\*\bkmkend bookmark_1}
\par
\pard
{\field{\*\fldinst { HYPERLINK  \\l "bookmark_1" }}{\fldrslt{click me}}}
\par
}"#;
        let doc = RtfExtractor::build_internal_document(rtf_content, true);
        for elem in &doc.elements {
            for ann in &elem.annotations {
                if let crate::types::document_structure::AnnotationKind::Link { ref url, .. } = ann.kind {
                    let link_text = &elem.text[ann.start as usize..ann.end as usize];
                    assert_eq!(
                        link_text, "click me",
                        "E2E: Hyperlink should cover 'click me', got: {:?}",
                        link_text
                    );
                    assert_eq!(url, "#bookmark_1");
                }
            }
        }
    }

    #[test]
    fn test_bookmark_hyperlink_rendered_markdown() {
        let rtf_content = r#"{\rtf1\ansi
\pard
{\*\bkmkstart bookmark_1}Bookmark_1{\*\bkmkend bookmark_1}
\par
\pard
{\field{\*\fldinst { HYPERLINK  \\l "bookmark_1" }}{\fldrslt{click me}}}
\par
}"#;
        let doc = RtfExtractor::build_internal_document(rtf_content, false);
        let rendered = crate::rendering::render_markdown(&doc);
        assert!(
            rendered.contains("[click me](#bookmark_1)"),
            "Should render as [click me](#bookmark_1), got: {:?}",
            rendered
        );
        assert!(
            !rendered.contains("[click m](#bookmark_1)e"),
            "Link boundary should not be off by one"
        );
    }

    #[test]
    fn test_fake_doc_table_extraction() {
        let rtf_content = r#"{\rtf1\ansi\deff0
{\pard \ql \f0 \sa180 \li0 \fi0 \outlinelevel0 \b \fs36 My First Heading\par}
{\pard \ql \f0 \sa180 \li0 \fi0 My first paragraph.\par}
{\pard \sa180 \li0 \fi0 \b Table Example:\par}
{\trowd\cellx3000\cellx6000
\pard\intbl\qc\fs20 Column 1\cell Column 2\cell\row
\pard\intbl\qc\fs20 Row 1, Cell 1\cell Row 1, Cell 2\cell\row
\pard\intbl\qc\fs20 Row 2, Cell 1\cell Row 2, Cell 2\cell\row
}
}"#;
        let (_text, tables, _, metas, _) = extract_text_from_rtf(rtf_content, true);
        assert!(!tables.is_empty(), "Should extract at least one table");
        assert_eq!(
            tables[0].cells.len(),
            3,
            "Table should have 3 rows (header + 2 data), got: {:?}",
            tables[0].cells
        );
        assert!(_text.contains("Table Example:"), "Should have 'Table Example:' in text");
        let table_metas: Vec<_> = metas.iter().filter(|m| m.is_table).collect();
        assert_eq!(
            table_metas.len(),
            3,
            "Should have 3 table row metas, got: {}",
            table_metas.len()
        );
    }

    #[test]
    fn test_multiple_tables_with_captions() {
        let rtf_content = r#"{\rtf1\ansi
{\pard First Caption\par}
{\trowd\cellx3000\cellx6000
\pard\intbl A1\cell A2\cell\row
\pard\intbl B1\cell B2\cell\row
}
{\pard Second Caption\par}
{\trowd\cellx3000\cellx6000
\pard\intbl C1\cell C2\cell\row
\pard\intbl D1\cell D2\cell\row
}
}"#;
        let (text, tables, _, _metas, _) = extract_text_from_rtf(rtf_content, true);
        assert_eq!(tables.len(), 2, "Should have 2 tables, got: {}", tables.len());
        assert_eq!(tables[0].cells.len(), 2, "First table: 2 rows");
        assert_eq!(tables[1].cells.len(), 2, "Second table: 2 rows");
        assert!(text.contains("First Caption"), "Should have 'First Caption'");
        assert!(text.contains("Second Caption"), "Should have 'Second Caption'");
    }

    #[test]
    fn test_listtext_ignorable_ordered_detection() {
        let rtf_content = r#"{\rtf1\ansi
\pard\ilvl0\ls1{\*\listtext 1.\tab}First item\par
\pard\ilvl0\ls1{\*\listtext 2.\tab}Second item\par
}"#;
        let (_text, _, _, metas, _) = extract_text_from_rtf(rtf_content, true);
        let ordered_items: Vec<_> = metas.iter().filter(|m| m.ordered && m.list_level.is_some()).collect();
        assert!(
            !ordered_items.is_empty(),
            "Should have ordered list items with \\* prefix, metas: {:?}",
            metas
        );
    }
}
