//! Native DOC extractor for Word 97-2003 binary format.
//!
//! Extracts text directly from OLE/CFB compound documents without LibreOffice.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::core::mime::LEGACY_WORD_MIME_TYPE;
use crate::extraction::doc::extract_doc_text;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::Metadata;
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
#[cfg_attr(alef, alef(skip))]
/// Native DOC extractor using OLE/CFB parsing.
///
/// This extractor handles Word 97-2003 binary (.doc) files without
/// requiring LibreOffice, providing ~50x faster extraction.
pub struct DocExtractor;

impl DocExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for DocExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for DocExtractor {
    fn name(&self) -> &str {
        "doc-extractor"
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
        "Native DOC text extraction via OLE/CFB parsing"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for DocExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
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
                    extract_doc_text(&content_owned)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("DOC extraction task failed: {e}")))?
            } else {
                extract_doc_text(content)
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                extract_doc_text(content)
            }
        }?;

        let mut doc = InternalDocument::new("doc");
        doc.mime_type = mime_type.to_string();
        doc.processing_warnings.extend(result.processing_warnings);

        let mut metadata_map = AHashMap::new();

        let meta_title = result.metadata.title;
        let meta_subject = result.metadata.subject;

        let (meta_authors, meta_created_by) = if let Some(author) = result.metadata.author {
            (Some(vec![author.clone()]), Some(author))
        } else {
            (None, None)
        };

        let meta_modified_by = result.metadata.last_author;

        if let Some(revision) = result.metadata.revision_number {
            metadata_map.insert(Cow::Borrowed("revision"), serde_json::Value::String(revision));
        }

        metadata_map.insert(
            Cow::Borrowed("extraction_method"),
            serde_json::Value::String("native_ole".to_string()),
        );

        doc.metadata = Metadata {
            title: meta_title,
            subject: meta_subject,
            authors: meta_authors,
            created_by: meta_created_by,
            modified_by: meta_modified_by,
            additional: metadata_map,
            ..Default::default()
        };

        let paragraphs: Vec<&str> = result.content.split("\n\n").collect();
        for (i, paragraph) in paragraphs.iter().enumerate() {
            let trimmed = paragraph.trim();
            if trimmed.is_empty() {
                continue;
            }

            let is_single_line = !trimmed.contains('\n');
            let is_short = trimmed.len() <= 80;
            let no_trailing_punct = !trimmed.ends_with('.') && !trimmed.ends_with(':') && !trimmed.ends_with(';');
            let next_is_longer = paragraphs.get(i + 1).is_some_and(|next| {
                let next_trimmed = next.trim();
                !next_trimmed.is_empty() && next_trimmed.len() > trimmed.len()
            });

            if is_single_line && is_short && no_trailing_punct && next_is_longer {
                doc.push_element(InternalElement::text(ElementKind::Heading { level: 2 }, trimmed, 0));
            } else {
                doc.push_element(InternalElement::text(ElementKind::Paragraph, trimmed, 0));
            }
        }

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[LEGACY_WORD_MIME_TYPE]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_doc_extractor_plugin_interface() {
        let extractor = DocExtractor::new();
        assert_eq!(extractor.name(), "doc-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert_eq!(extractor.supported_mime_types(), &["application/msword"]);
    }

    #[tokio::test]
    async fn test_doc_extractor_initialize_shutdown() {
        let extractor = DocExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[tokio::test]
    async fn test_doc_extractor_real_file() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/unstructured/doc/simple.doc");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test DOC");
        let extractor = DocExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(&content, "application/msword", &config)
            .await
            .expect("DOC extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(!result.content.is_empty(), "Should extract text from DOC");
        assert_eq!(&*result.mime_type, "application/msword");
    }

    #[tokio::test]
    async fn test_doc_document_structure_with_heuristic_headings() {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/vendored/unstructured/doc/simple.doc");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test DOC");
        let extractor = DocExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(&content, "application/msword", &config)
            .await
            .expect("DOC extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(result.document.is_some(), "Should produce document structure for DOC");
        let doc = result.document.unwrap();
        assert!(!doc.nodes.is_empty(), "Document structure should have nodes");
    }

    #[tokio::test]
    async fn test_doc_paragraph_mapping() {
        let test_file =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/doc/unit_test_lists.doc");
        if !test_file.exists() {
            return;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test DOC");
        let extractor = DocExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(&content, "application/msword", &config)
            .await
            .expect("DOC extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        assert!(result.document.is_some(), "Should produce document structure");
        let doc = result.document.unwrap();
        let has_paragraph = doc.nodes.iter().any(|n| {
            matches!(
                n.content,
                crate::types::document_structure::NodeContent::Paragraph { .. }
            )
        });
        assert!(has_paragraph, "DOC should produce Paragraph nodes");
    }
}
