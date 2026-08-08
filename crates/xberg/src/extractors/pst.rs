//! PST (Outlook Personal Folders) extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::transform::normalize_line_endings;
use crate::extractors::SyncExtractor;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::metadata::PstMetadata;
use crate::types::{FormatMetadata, Metadata};
use async_trait::async_trait;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

/// PST file extractor.
///
/// Supports: .pst (Microsoft Outlook Personal Folders)
#[cfg_attr(alef, alef(skip))]
pub struct PstExtractor;

impl Default for PstExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PstExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Plugin for PstExtractor {
    fn name(&self) -> &str {
        "pst-extractor"
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

impl SyncExtractor for PstExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, _config: &ExtractionConfig) -> Result<InternalDocument> {
        let (messages, processing_warnings) = crate::extraction::pst::extract_pst_messages(content)?;

        let mut doc = InternalDocument::new("pst");
        doc.mime_type = mime_type.to_string();

        push_messages_as_elements(&mut doc, &messages);

        let (subject, created_at) = if let Some(first) = messages.first() {
            (first.subject.clone(), first.date.clone())
        } else {
            (None, None)
        };

        let pst_metadata = PstMetadata {
            message_count: messages.len(),
        };

        doc.metadata = Metadata {
            format: Some(FormatMetadata::Pst(pst_metadata)),
            subject,
            created_at,
            ..Default::default()
        };

        doc.metadata.additional.insert(
            std::borrow::Cow::Borrowed("message_count"),
            serde_json::json!(messages.len()),
        );

        merge_pst_warnings(&mut doc, processing_warnings);

        Ok(doc)
    }
}

/// Merge PST parse/traversal warnings into a document's `processing_warnings`.
///
/// Extracted as a small standalone helper so warning propagation (issue #151)
/// can be unit tested against a synthetic warnings vector without needing to
/// open a real PST file.
fn merge_pst_warnings(doc: &mut InternalDocument, warnings: Vec<crate::types::ProcessingWarning>) {
    doc.processing_warnings.extend(warnings);
}

/// Flatten messages into paragraph elements, emitting a heading whenever the
/// message's PST folder path (tracked in `EmailExtractionResult::metadata`
/// under the `folder_path` key) changes from the previous message, so the
/// folder hierarchy is visible in the output instead of a flat list of
/// unlabeled paragraphs.
fn push_messages_as_elements(doc: &mut InternalDocument, messages: &[crate::types::EmailExtractionResult]) {
    let mut last_folder_path: Option<&str> = None;

    for msg in messages {
        let folder_path = msg.metadata.get("folder_path").map(String::as_str);
        if let Some(path) = folder_path
            && last_folder_path != Some(path)
        {
            doc.push_element(InternalElement::text(ElementKind::Heading { level: 1 }, path, 0));
            last_folder_path = Some(path);
        }

        // `build_email_text_output` embeds `EmailExtractionResult::content` verbatim,
        // and for PST that content is `PR_BODY` (0x1000) straight out of the message
        // store — Outlook writes CRLF there (see `resolve_pst_body` in
        // `extraction/pst.rs`, which passes the plain-text property through untouched).
        // Normalize before splitting or the whole message body becomes one paragraph (#316).
        let msg_text = normalize_line_endings(&crate::extraction::email::build_email_text_output(msg)).into_owned();
        if !msg_text.is_empty() {
            for paragraph in msg_text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    doc.push_element(InternalElement::text(ElementKind::Paragraph, trimmed, 0));
                }
            }
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PstExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let mut doc = self.extract_sync(content, mime_type, config)?;

        // Note: `extract_sync` above already parses `content` once and merges its
        // parse warnings into `doc.processing_warnings` (see #151). The second parse
        // here exists only to obtain attachments for child-document extraction, so
        // its own parse warnings are intentionally not re-merged (they would be
        // exact duplicates of what `extract_sync` already recorded).
        if config.max_archive_depth > 0
            && let Ok((messages, _parse_warnings)) = crate::extraction::pst::extract_pst_messages(content)
        {
            let all_attachments: Vec<_> = messages.iter().flat_map(|m| m.attachments.iter()).cloned().collect();
            let (children, attachment_warnings) =
                crate::extractors::email::extract_attachment_children(&all_attachments, config).await;
            if !children.is_empty() {
                doc.children = Some(children);
            }
            merge_pst_warnings(&mut doc, attachment_warnings);
        }

        Ok(doc)
    }

    #[cfg(feature = "tokio-runtime")]
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, path, config),
        fields(
            extractor.name = self.name(),
        )
    ))]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let (messages, processing_warnings) = crate::extraction::pst::extract_pst_from_path(path)?;

        let mut doc = InternalDocument::new("pst");
        doc.mime_type = mime_type.to_string();

        push_messages_as_elements(&mut doc, &messages);

        let (subject, created_at) = if let Some(first) = messages.first() {
            (first.subject.clone(), first.date.clone())
        } else {
            (None, None)
        };

        let pst_metadata = PstMetadata {
            message_count: messages.len(),
        };

        doc.metadata = crate::types::Metadata {
            format: Some(FormatMetadata::Pst(pst_metadata)),
            subject,
            created_at,
            ..Default::default()
        };

        doc.metadata.additional.insert(
            std::borrow::Cow::Borrowed("message_count"),
            serde_json::json!(messages.len()),
        );

        merge_pst_warnings(&mut doc, processing_warnings);

        if config.max_archive_depth > 0 {
            let all_attachments: Vec<_> = messages.iter().flat_map(|m| m.attachments.iter()).cloned().collect();
            let (children, attachment_warnings) =
                crate::extractors::email::extract_attachment_children(&all_attachments, config).await;
            if !children.is_empty() {
                doc.children = Some(children);
            }
            merge_pst_warnings(&mut doc, attachment_warnings);
        }

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/vnd.ms-outlook-pst"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pst_extractor_plugin_interface() {
        let extractor = PstExtractor::new();
        assert_eq!(extractor.name(), "pst-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_pst_extractor_supported_mime_types() {
        let extractor = PstExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 1);
        assert!(mime_types.contains(&"application/vnd.ms-outlook-pst"));
    }

    #[test]
    fn test_pst_extractor_invalid_data() {
        let config = ExtractionConfig::default();
        let extractor = PstExtractor::new();
        let result = extractor.extract_sync(b"not a pst file", "application/vnd.ms-outlook-pst", &config);
        assert!(result.is_err());
    }

    fn sample_message(folder_path: Option<&str>, content: &str) -> crate::types::EmailExtractionResult {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("entry_id".to_string(), "00".to_string());
        if let Some(path) = folder_path {
            metadata.insert("folder_path".to_string(), path.to_string());
        }

        crate::types::EmailExtractionResult {
            subject: Some("Subject".to_string()),
            from_email: None,
            to_emails: Vec::new(),
            cc_emails: Vec::new(),
            bcc_emails: Vec::new(),
            date: None,
            message_id: None,
            plain_text: Some(content.to_string()),
            html_content: None,
            content: content.to_string(),
            attachments: Vec::new(),
            metadata,
        }
    }

    /// Regression test for issue #151: every warning collected while parsing
    /// the PST must end up in the final document's `processing_warnings`,
    /// none may be silently dropped.
    #[test]
    fn test_merge_pst_warnings_propagates_every_warning_into_document() {
        let mut doc = InternalDocument::new("pst");
        let warnings = vec![
            crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pst_extraction"),
                message: std::borrow::Cow::Borrowed("warning one"),
            },
            crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pst_extraction"),
                message: std::borrow::Cow::Borrowed("warning two"),
            },
        ];

        merge_pst_warnings(&mut doc, warnings);

        assert_eq!(doc.processing_warnings.len(), 2);
        assert_eq!(doc.processing_warnings[0].message.as_ref(), "warning one");
        assert_eq!(doc.processing_warnings[1].message.as_ref(), "warning two");
    }

    #[test]
    fn test_merge_pst_warnings_is_additive_across_multiple_calls() {
        let mut doc = InternalDocument::new("pst");
        merge_pst_warnings(
            &mut doc,
            vec![crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pst_extraction"),
                message: std::borrow::Cow::Borrowed("first batch"),
            }],
        );
        merge_pst_warnings(
            &mut doc,
            vec![crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("pst_extraction"),
                message: std::borrow::Cow::Borrowed("second batch"),
            }],
        );

        assert_eq!(doc.processing_warnings.len(), 2);
        assert_eq!(doc.processing_warnings[0].message.as_ref(), "first batch");
        assert_eq!(doc.processing_warnings[1].message.as_ref(), "second batch");
    }

    /// Regression test for issue #162(c)/heading requirement: messages must be
    /// grouped under a `Heading` element for their PST folder path, and the
    /// heading must only be emitted once per contiguous run of the same folder.
    #[test]
    fn test_push_messages_as_elements_emits_heading_per_folder_change() {
        let mut doc = InternalDocument::new("pst");
        let messages = vec![
            sample_message(Some("Top of Personal Folders/Inbox"), "first message body"),
            sample_message(Some("Top of Personal Folders/Inbox"), "second message body"),
            sample_message(Some("Top of Personal Folders/Sent Items"), "third message body"),
        ];

        push_messages_as_elements(&mut doc, &messages);

        let headings: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Heading { .. }))
            .map(|e| e.text.as_ref())
            .collect();

        assert_eq!(
            headings,
            vec!["Top of Personal Folders/Inbox", "Top of Personal Folders/Sent Items"],
            "one heading per contiguous folder-path run, not one per message"
        );
    }

    #[test]
    fn test_push_messages_as_elements_no_heading_when_folder_path_absent() {
        let mut doc = InternalDocument::new("pst");
        let messages = vec![sample_message(None, "body with no folder path")];

        push_messages_as_elements(&mut doc, &messages);

        assert!(
            doc.elements
                .iter()
                .all(|e| !matches!(e.kind, ElementKind::Heading { .. })),
            "no heading should be emitted when a message has no folder_path metadata"
        );
    }
}
