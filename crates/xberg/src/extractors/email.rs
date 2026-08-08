//! Email message extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::transform::normalize_line_endings;
use crate::extractors::SyncExtractor;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use crate::types::{ArchiveEntry, EmailMetadata, ProcessingWarning};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

const EMAIL_STRUCT_KEYS: &[&str] = &[
    "from_email",
    "from_name",
    "to_emails",
    "cc_emails",
    "bcc_emails",
    "message_id",
    "attachments",
    "subject",
    "date",
    "email_from",
    "email_to",
    "email_cc",
    "email_bcc",
];
const INLINED_ATTACHMENT_ELEMENT_COUNT: usize = 2;
#[cfg_attr(alef, alef(skip))]
/// Email message extractor.
///
/// Supports: .eml, .msg
pub struct EmailExtractor;

impl Default for EmailExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl EmailExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl EmailExtractor {
    /// Build an `InternalDocument` from extracted email content.
    ///
    /// Pushes email headers as a metadata block, body content as paragraphs, and
    /// successful nonblank attachment content under level-two headings.
    fn build_internal_document(
        email_result: &crate::types::EmailExtractionResult,
        extracted_attachments: &[ArchiveEntry],
    ) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("email");

        let mut header_entries = Vec::new();
        if let Some(ref subject) = email_result.subject {
            header_entries.push(("Subject".to_string(), subject.clone()));
        }
        if let Some(from) = Self::format_sender(email_result) {
            header_entries.push(("From".to_string(), from));
        }
        if !email_result.to_emails.is_empty() {
            header_entries.push(("To".to_string(), email_result.to_emails.join(", ")));
        }
        if !email_result.cc_emails.is_empty() {
            header_entries.push(("CC".to_string(), email_result.cc_emails.join(", ")));
        }
        if !email_result.bcc_emails.is_empty() {
            header_entries.push(("BCC".to_string(), email_result.bcc_emails.join(", ")));
        }
        if let Some(reply_to) = email_result.metadata.get("reply_to") {
            header_entries.push(("Reply-To".to_string(), reply_to.clone()));
        }
        if let Some(ref date) = email_result.date {
            header_entries.push(("Date".to_string(), date.clone()));
        }
        if let Some(ref message_id) = email_result.message_id {
            header_entries.push(("Message-ID".to_string(), message_id.clone()));
        }
        if let Some(in_reply_to) = email_result.metadata.get("in_reply_to") {
            header_entries.push(("In-Reply-To".to_string(), in_reply_to.clone()));
        }
        if let Some(references) = email_result.metadata.get("references") {
            header_entries.push(("References".to_string(), references.clone()));
        }
        if let Some(list_id) = email_result.metadata.get("list_id") {
            header_entries.push(("List-Id".to_string(), list_id.clone()));
        }
        if let Some(list_unsubscribe) = email_result.metadata.get("list_unsubscribe") {
            header_entries.push(("List-Unsubscribe".to_string(), list_unsubscribe.clone()));
        }
        if !header_entries.is_empty() {
            builder.push_metadata_block(&header_entries, None);
        }

        if let Some(ref html) = email_result.html_content {
            let html_doc = crate::extraction::html::structure::build_document_structure(html);
            for (idx, node) in html_doc.nodes.iter().enumerate() {
                if node.parent.is_none() {
                    process_node(&html_doc, idx, &mut builder);
                }
            }
        } else {
            // RFC 5322 mandates CRLF line endings, and `mail-parser` hands back the
            // decoded body verbatim (both the 7bit/8bit and quoted-printable decoders
            // preserve `\r\n`). Without normalizing first, `\r\n\r\n` never matches the
            // `"\n\n"` paragraph boundary and every plain-text email collapses into a
            // single paragraph carrying stray carriage returns (#316, same defect class
            // as #227).
            let normalized = normalize_line_endings(&email_result.content);
            for paragraph in normalized.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    builder.push_paragraph(trimmed, vec![], None, None);
                }
            }
        }

        if !email_result.attachments.is_empty() {
            builder.push_paragraph("Attachments:", vec![], None, None);
            for att in &email_result.attachments {
                let name = att.filename.as_deref().or(att.name.as_deref()).unwrap_or("unnamed");
                let size = att.size.unwrap_or(0);
                let att_text = format!("  {} ({}B)", name, size);
                builder.push_paragraph(&att_text, vec![], None, None);
            }
        }

        for attachment in extracted_attachments {
            let content = attachment.result.content.trim();
            if content.is_empty() {
                continue;
            }
            builder.push_heading(2, &attachment.path, None, None);
            builder.push_paragraph(content, vec![], None, None);
        }

        builder.build()
    }

    /// Preserve both parts of a mailbox while avoiding `address <address>`.
    fn format_sender(email_result: &crate::types::EmailExtractionResult) -> Option<String> {
        let name = email_result
            .metadata
            .get("from_name")
            .filter(|name| !name.trim().is_empty());
        match (name, email_result.from_email.as_ref()) {
            (Some(name), Some(address)) if !name.eq_ignore_ascii_case(address) => Some(format!("{name} <{address}>")),
            (_, Some(address)) => Some(address.clone()),
            (Some(name), None) => Some(name.clone()),
            (None, None) => None,
        }
    }

    /// Convert one parsed email into the internal representation.
    fn build_extracted_document(
        email_result: &crate::types::EmailExtractionResult,
        mime_type: &str,
        config: &ExtractionConfig,
        extracted_attachments: &[ArchiveEntry],
    ) -> Result<InternalDocument> {
        let mut doc = Self::build_internal_document(email_result, extracted_attachments);
        doc.mime_type = mime_type.to_string();
        doc.metadata = Self::build_document_metadata(email_result);

        let mut budget = SecurityBudget::from_config(config);
        retain_budgeted_attachment_content(&mut doc, extracted_attachments, &mut budget)?;

        Ok(doc)
    }

    fn build_document_metadata(email_result: &crate::types::EmailExtractionResult) -> Metadata {
        let attachment_names = email_result
            .attachments
            .iter()
            .filter_map(|attachment| attachment.filename.clone().or_else(|| attachment.name.clone()))
            .collect();
        let additional = email_result
            .metadata
            .iter()
            .filter(|(key, _)| !EMAIL_STRUCT_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (Cow::Owned(key.clone()), serde_json::json!(value)))
            .collect::<AHashMap<_, _>>();
        let from_name = email_result.metadata.get("from_name").cloned();
        let email_metadata = EmailMetadata {
            from_email: email_result.from_email.clone(),
            from_name: from_name.clone(),
            to_emails: email_result.to_emails.clone(),
            cc_emails: email_result.cc_emails.clone(),
            bcc_emails: email_result.bcc_emails.clone(),
            message_id: email_result.message_id.clone(),
            attachments: attachment_names,
        };

        let authors = from_name.filter(|name| !name.is_empty()).map(|name| vec![name]);

        Metadata {
            format: Some(crate::types::FormatMetadata::Email(email_metadata)),
            subject: email_result.subject.clone(),
            authors,
            created_at: email_result.date.clone(),
            additional,
            ..Default::default()
        }
    }
}

fn retain_budgeted_attachment_content(
    doc: &mut InternalDocument,
    extracted_attachments: &[ArchiveEntry],
    budget: &mut SecurityBudget,
) -> Result<()> {
    let inlined_attachment_count = extracted_attachments
        .iter()
        .filter(|attachment| !attachment.result.content.trim().is_empty())
        .count();
    let attachment_element_count = inlined_attachment_count.saturating_mul(INLINED_ATTACHMENT_ELEMENT_COUNT);
    let attachment_elements = doc
        .elements
        .split_off(doc.elements.len().saturating_sub(attachment_element_count));

    for element in &doc.elements {
        budget.account_text(element.text.len())?;
    }

    for (attachment, elements) in extracted_attachments
        .iter()
        .filter(|attachment| !attachment.result.content.trim().is_empty())
        .zip(attachment_elements.chunks_exact(INLINED_ATTACHMENT_ELEMENT_COUNT))
    {
        let mut candidate_budget = budget.clone();
        let fits_budget = elements
            .iter()
            .all(|element| candidate_budget.account_text(element.text.len()).is_ok());
        if fits_budget {
            *budget = candidate_budget;
            doc.elements.extend_from_slice(elements);
        } else {
            doc.processing_warnings.push(ProcessingWarning {
                source: Cow::Borrowed("email_attachment_inlining"),
                message: Cow::Owned(format!(
                    "Skipped inlining attachment '{}': extracted text exceeds the remaining content security limit",
                    attachment.path
                )),
            });
        }
    }

    Ok(())
}

/// Recursively process a `DocumentNode` and its children into the `InternalDocumentBuilder`.
fn process_node(
    doc: &crate::types::document_structure::DocumentStructure,
    node_idx: usize,
    builder: &mut InternalDocumentBuilder,
) {
    if let Some(node) = doc.nodes.get(node_idx) {
        match &node.content {
            crate::types::NodeContent::Paragraph { text } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    builder.push_paragraph(trimmed, node.annotations.clone(), None, None);
                }
            }
            crate::types::NodeContent::Heading { level, text } => {
                builder.push_heading(*level, text.as_str(), None, None);
            }
            crate::types::NodeContent::Title { text } => {
                builder.push_title(text.as_str(), None, None);
            }
            crate::types::NodeContent::List { ordered } => {
                builder.push_list(*ordered);
                for &child_idx in &node.children {
                    process_node(doc, child_idx.0 as usize, builder);
                }
                builder.end_list();
            }
            crate::types::NodeContent::ListItem { text } => {
                let ordered = if let Some(parent_idx) = node.parent
                    && let Some(parent) = doc.nodes.get(parent_idx.0 as usize)
                    && let crate::types::NodeContent::List { ordered } = parent.content
                {
                    ordered
                } else {
                    false
                };
                builder.push_list_item(text.as_str(), ordered, node.annotations.clone(), None, None);

                for &child_idx in &node.children {
                    process_node(doc, child_idx.0 as usize, builder);
                }
            }
            crate::types::NodeContent::Table { grid } => {
                let rows = crate::extraction::grid_flatten::flatten_positioned_cells(
                    grid.rows as usize,
                    grid.cells
                        .iter()
                        .map(|c| (c.row, c.row_span, c.col_span, c.content.clone())),
                );
                builder.push_table_from_cells(&rows, None, None);
            }
            crate::types::NodeContent::Code { text, language } => {
                builder.push_code(text.as_str(), language.as_deref(), None, None);
            }
            crate::types::NodeContent::Formula { text } => {
                builder.push_formula(text.as_str(), None, None);
            }
            crate::types::NodeContent::MetadataBlock { entries } => {
                builder.push_metadata_block(entries, None);
            }
            crate::types::NodeContent::Quote => {
                builder.push_quote_start();
                for &child_idx in &node.children {
                    process_node(doc, child_idx.0 as usize, builder);
                }
                builder.push_quote_end();
            }
            crate::types::NodeContent::Group { label, .. } => {
                builder.push_group_start(label.as_deref(), None);
                for &child_idx in &node.children {
                    process_node(doc, child_idx.0 as usize, builder);
                }
                builder.push_group_end();
            }
            crate::types::NodeContent::Admonition { kind, title } => {
                builder.push_admonition(kind, title.as_deref(), None);
                for &child_idx in &node.children {
                    process_node(doc, child_idx.0 as usize, builder);
                }
            }
            crate::types::NodeContent::Image { description, .. } => {
                let text = description.as_deref().unwrap_or("[Image]");
                builder.push_paragraph(text, vec![], None, None);
            }
            _ => {
                if let Some(text) = node.content.text() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        builder.push_paragraph(trimmed, node.annotations.clone(), None, None);
                    }
                }
            }
        }
    }
}

impl Plugin for EmailExtractor {
    fn name(&self) -> &str {
        "email-extractor"
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

impl SyncExtractor for EmailExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let fallback_codepage = config.email.as_ref().and_then(|e| e.msg_fallback_codepage);
        let email_result = crate::extraction::email::extract_email_content(content, mime_type, fallback_codepage)?;
        Self::build_extracted_document(&email_result, mime_type, config, &[])
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for EmailExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "email", size_bytes = content.len(), "extraction starting");
        let fallback_codepage = config.email.as_ref().and_then(|email| email.msg_fallback_codepage);
        let security_limits = config.security_limits.clone().unwrap_or_default();
        let parsed = if config.max_archive_depth > 0 {
            crate::extraction::email::extract_email_content_with_nested(
                content,
                mime_type,
                fallback_codepage,
                config.max_embedded_file_bytes,
                &security_limits,
            )?
        } else {
            crate::extraction::email::ParsedEmailContent {
                result: crate::extraction::email::extract_email_content(content, mime_type, fallback_codepage)?,
                nested_messages: Vec::new(),
                nested_embedded_messages: Vec::new(),
                warnings: Vec::new(),
            }
        };
        let mut children = Vec::new();
        let mut warnings = parsed.warnings;

        if config.max_archive_depth > 0 {
            let (attachment_children, attachment_warnings) =
                extract_attachment_children(&parsed.result.attachments, config).await;
            children = attachment_children;
            warnings.extend(attachment_warnings);

            // An `afEmbeddedMessage` attachment is an attachment of the parent
            // message, so its recovered text is inlined into the parent exactly
            // like any other extracted attachment (heading + body, accounted
            // against the same `SecurityBudget`). This must happen *before*
            // `build_extracted_document`, which is what performs the inlining;
            // appending afterwards would leave the embedded body reachable only
            // through `doc.children` and absent from the parent's rendered
            // content (#307, #153).
            if !parsed.nested_embedded_messages.is_empty() {
                let (embedded_children, embedded_warnings) =
                    extract_nested_embedded_message_children(&parsed.nested_embedded_messages, config).await;
                children.extend(embedded_children);
                warnings.extend(embedded_warnings);
            }
        }

        let mut doc = Self::build_extracted_document(&parsed.result, mime_type, config, &children)?;

        // MIME `message/rfc822` sub-parts are a body structure, not an
        // attachment, so they stay children-only and are deliberately not
        // inlined into the parent's elements.
        if config.max_archive_depth > 0 && mime_type == "message/rfc822" {
            let (nested_children, nested_warnings) =
                extract_nested_message_children(&parsed.nested_messages, config).await;
            children.extend(nested_children);
            warnings.extend(nested_warnings);
        }

        if !children.is_empty() {
            doc.children = Some(children);
        }
        doc.processing_warnings.extend(warnings);

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "email",
            "extraction complete"
        );
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
        let bytes = crate::core::io::read_file_async(path).await?;
        self.extract_content(&bytes, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["message/rfc822", "application/vnd.ms-outlook"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Recursively extract content from email attachments.
///
/// For each attachment with binary data, detects its MIME type and dispatches
/// to the appropriate extractor. Follows the same pattern as archive extractors.
/// Errors on individual attachments are captured as warnings, never failing
/// the whole extraction.
pub(crate) async fn extract_attachment_children(
    attachments: &[crate::types::EmailAttachment],
    config: &ExtractionConfig,
) -> (Vec<ArchiveEntry>, Vec<ProcessingWarning>) {
    let mut children = Vec::new();
    let mut warnings = Vec::new();

    for (idx, attachment) in attachments.iter().enumerate() {
        if attachment
            .mime_type
            .as_deref()
            .is_some_and(|mime_type| mime_type.eq_ignore_ascii_case("message/rfc822"))
        {
            continue;
        }

        let filename = attachment
            .filename
            .clone()
            .or_else(|| attachment.name.clone())
            .unwrap_or_else(|| format!("attachment_{}", idx));

        let bytes = match &attachment.data {
            Some(data) if !data.is_empty() => data,
            _ => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("email_attachment_extraction"),
                    message: Cow::Owned(format!(
                        "Skipped attachment '{}': no attachment data available",
                        filename
                    )),
                });
                continue;
            }
        };

        let detected_mime = crate::core::mime::detect_mime_type_from_bytes(bytes)
            .ok()
            .or_else(|| {
                std::path::Path::new(&filename)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(|ext| mime_guess::from_ext(ext).first())
                    .map(|m| m.to_string())
            })
            .or_else(|| attachment.mime_type.clone().filter(|m| m != "application/octet-stream"));

        let file_mime = match detected_mime {
            Some(m) if m != "application/octet-stream" => m,
            _ => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("email_attachment_extraction"),
                    message: Cow::Owned(format!(
                        "Skipped attachment '{}': could not determine MIME type",
                        filename
                    )),
                });
                continue;
            }
        };

        if config
            .max_embedded_file_bytes
            .is_some_and(|cap| bytes.len() as u64 > cap)
        {
            let cap = config.max_embedded_file_bytes.unwrap_or(0);
            warnings.push(ProcessingWarning {
                source: Cow::Borrowed("email_attachment_extraction"),
                message: Cow::Owned(format!(
                    "Skipped attachment '{}': size {} bytes exceeds cap {} bytes",
                    filename,
                    bytes.len(),
                    cap
                )),
            });
            continue;
        }

        let mut child_config = config.clone();
        child_config.max_archive_depth = config.max_archive_depth.saturating_sub(1);

        match crate::core::extractor::extract_bytes(bytes, &file_mime, &child_config).await {
            Ok(result) => {
                children.push(ArchiveEntry {
                    path: filename,
                    mime_type: file_mime,
                    result: Box::new(result),
                });
            }
            Err(e) => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("email_attachment_extraction"),
                    message: Cow::Owned(format!("Failed to extract '{}': {}", filename, e)),
                });
            }
        }
    }

    (children, warnings)
}

/// Extract nested `message/rfc822` sub-messages as `ArchiveEntry` children.
///
/// Recursively extracts `PartType::Message` payloads retained by the initial
/// top-level EML parse via
/// `extract_bytes` with `message/rfc822` MIME type. This makes nested
/// messages available as separate children for recursive processing,
/// complementing the existing text-merging in `collect_nested_message_text`
/// / `collect_nested_message_html`.
async fn extract_nested_message_children(
    nested_messages: &[crate::extraction::email::NestedMessagePayload],
    config: &ExtractionConfig,
) -> (Vec<ArchiveEntry>, Vec<ProcessingWarning>) {
    let mut children = Vec::new();
    let mut warnings = Vec::new();

    for (nested_idx, payload) in nested_messages.iter().enumerate() {
        let filename = format!("nested_message_{nested_idx}.eml");

        let Some(raw_bytes) = payload.data.as_ref() else {
            let cap = config.max_embedded_file_bytes.unwrap_or(0);
            warnings.push(ProcessingWarning {
                source: Cow::Borrowed("nested_message_extraction"),
                message: Cow::Owned(format!(
                    "Skipped '{}': size {} bytes exceeds cap {} bytes",
                    filename, payload.size, cap
                )),
            });
            continue;
        };

        let mut child_config = config.clone();
        child_config.max_archive_depth = config.max_archive_depth.saturating_sub(1);

        match crate::core::extractor::extract_bytes(raw_bytes, "message/rfc822", &child_config).await {
            Ok(result) => {
                children.push(ArchiveEntry {
                    path: filename,
                    mime_type: "message/rfc822".to_string(),
                    result: Box::new(result),
                });
            }
            Err(e) => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("nested_message_extraction"),
                    message: Cow::Owned(format!("Failed to extract '{}': {}", filename, e)),
                });
            }
        }
    }

    (children, warnings)
}

/// Recursively route `.msg`-in-`.msg` embedded messages (`afEmbeddedMessage`
/// attachments) through this same extractor, producing a real `ArchiveEntry`
/// child (subject, body, its own attachments) instead of leaving the embedded
/// message as an opaque, unextracted attachment placeholder.
///
/// The returned entries are handed to `build_extracted_document` alongside the
/// ordinary attachment children, so the embedded message's text is inlined into
/// the parent document under a level-two heading as well as being reachable as
/// a child.
///
/// `nested_embedded_messages` is the flat list `extraction::email` already
/// parsed while walking the CFB tree, bounded by the `SecurityLimits`
/// nesting-depth cap shared with every other format (see
/// `extraction::email::extract_msg_from_cfb_at`); a cap warning it raised is
/// already carried in `ParsedEmailContent::warnings`, so this function only
/// ever re-extracts messages that were actually parsed.
async fn extract_nested_embedded_message_children(
    nested_embedded_messages: &[crate::types::EmailExtractionResult],
    config: &ExtractionConfig,
) -> (Vec<ArchiveEntry>, Vec<ProcessingWarning>) {
    let mut children = Vec::new();
    let mut warnings = Vec::new();

    for (idx, nested) in nested_embedded_messages.iter().enumerate() {
        let filename = format!("embedded_message_{idx}.msg");

        let mut child_config = config.clone();
        child_config.max_archive_depth = config.max_archive_depth.saturating_sub(1);

        let (grandchildren, grandchild_warnings) =
            extract_attachment_children(&nested.attachments, &child_config).await;
        warnings.extend(grandchild_warnings);

        let mut internal_doc = match EmailExtractor::build_extracted_document(
            nested,
            "application/vnd.ms-outlook",
            &child_config,
            &grandchildren,
        ) {
            Ok(doc) => doc,
            Err(e) => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("msg_embedded_message_extraction"),
                    message: Cow::Owned(format!("Failed to extract embedded message '{}': {}", filename, e)),
                });
                continue;
            }
        };
        // The embedded message's own attachments are inlined into its text by
        // `build_extracted_document`; keep them addressable as structured
        // children too, exactly as a top-level message's attachments are.
        if !grandchildren.is_empty() {
            internal_doc.children = Some(grandchildren);
        }

        match crate::core::pipeline::run_pipeline(internal_doc, &child_config).await {
            Ok(result) => {
                children.push(ArchiveEntry {
                    path: filename,
                    mime_type: "application/vnd.ms-outlook".to_string(),
                    result: Box::new(result),
                });
            }
            Err(e) => {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("msg_embedded_message_extraction"),
                    message: Cow::Owned(format!("Failed to extract embedded message '{}': {}", filename, e)),
                });
            }
        }
    }

    (children, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED_MESSAGE_EML: &[u8] = b"From: digest@example.com\r\n\
Subject: Digest\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/digest; boundary=digest\r\n\
\r\n\
--digest\r\n\
Content-Type: message/rfc822\r\n\
\r\n\
From: child@example.com\r\n\
Subject: Nested\r\n\
\r\n\
Nested body\r\n\
--digest--\r\n";

    const IMPLICIT_NESTED_MESSAGE_EML: &[u8] = b"From: digest@example.com\r\n\
Subject: Digest\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/digest; boundary=digest\r\n\
\r\n\
--digest\r\n\
\r\n\
From: child@example.com\r\n\
Subject: Implicit Nested\r\n\
\r\n\
Implicit nested body\r\n\
--digest--\r\n";

    const TEXT_ATTACHMENT_EML: &[u8] = b"From: Alice <alice@example.com>\r\n\
Subject: Attachment\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=part\r\n\
\r\n\
--part\r\n\
Content-Type: text/plain\r\n\
\r\n\
Parent body\r\n\
--part\r\n\
Content-Type: text/plain\r\n\
Content-Disposition: attachment; filename=note.txt\r\n\
\r\n\
Attachment body\r\n\
--part--\r\n";

    #[test]
    fn test_email_extractor_plugin_interface() {
        let extractor = EmailExtractor::new();
        assert_eq!(extractor.name(), "email-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_email_extractor_supported_mime_types() {
        let extractor = EmailExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 2);
        assert!(mime_types.contains(&"message/rfc822"));
        assert!(mime_types.contains(&"application/vnd.ms-outlook"));
    }

    #[test]
    fn test_email_extractor_uses_config() {
        use crate::core::config::EmailConfig;

        let config = ExtractionConfig {
            email: Some(EmailConfig {
                msg_fallback_codepage: Some(1251),
            }),
            ..Default::default()
        };
        let extractor = EmailExtractor::new();
        let result = extractor.extract_sync(b"", "application/vnd.ms-outlook", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sender_name_and_address_are_rendered_once() {
        let eml = b"From: Alice Example <alice@example.com>\r\nSubject: Sender\r\n\r\nBody";
        let doc = EmailExtractor::new()
            .extract_sync(eml, "message/rfc822", &ExtractionConfig::default())
            .unwrap();
        let from_values: Vec<&str> = doc
            .elements
            .iter()
            .filter(|element| matches!(element.kind, crate::types::internal::ElementKind::MetadataBlock))
            .flat_map(|element| element.attributes.iter())
            .flat_map(|attributes| attributes.values())
            .map(String::as_str)
            .filter(|value| value.contains("alice@example.com"))
            .collect();

        assert_eq!(from_values, vec!["Alice Example <alice@example.com>"]);
        let crate::types::FormatMetadata::Email(email) = doc.metadata.format.unwrap() else {
            panic!("expected email metadata");
        };
        assert_eq!(email.from_name.as_deref(), Some("Alice Example"));
        assert_eq!(email.from_email.as_deref(), Some("alice@example.com"));
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_msg_preserves_sync_output_prefix_when_extracting_attachments() {
        let data = include_bytes!("../../../../test_documents/email/attachment.msg");
        let config = ExtractionConfig {
            max_archive_depth: 1,
            ..Default::default()
        };
        let extractor = EmailExtractor::new();

        let sync_doc = extractor
            .extract_sync(data, "application/vnd.ms-outlook", &config)
            .unwrap();
        let async_doc = extractor
            .extract_content(data, "application/vnd.ms-outlook", &config)
            .await
            .unwrap();

        assert_eq!(&async_doc.elements[..sync_doc.elements.len()], sync_doc.elements);
        assert_eq!(
            serde_json::to_value(&async_doc.metadata).unwrap(),
            serde_json::to_value(&sync_doc.metadata).unwrap()
        );
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_email_preserves_extractable_attachment_children() {
        let config = ExtractionConfig {
            max_archive_depth: 1,
            ..Default::default()
        };

        let doc = EmailExtractor::new()
            .extract_content(TEXT_ATTACHMENT_EML, "message/rfc822", &config)
            .await
            .unwrap();
        let plain = crate::rendering::render_plain(&doc);
        let markdown = crate::rendering::render_markdown(&doc);
        let children = doc.children.as_ref().expect("text attachment should be extracted");

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "note.txt");
        assert!(children[0].result.content.contains("Attachment body"));
        assert!(markdown.contains("## note.txt"));
        assert!(plain.contains(children[0].result.content.trim()));
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_email_skips_attachment_inline_when_combined_content_exceeds_limit() {
        let config = ExtractionConfig {
            max_archive_depth: 1,
            security_limits: Some(crate::extractors::security::SecurityLimits {
                max_content_size: 100,
                ..Default::default()
            }),
            ..Default::default()
        };

        let doc = EmailExtractor::new()
            .extract_content(TEXT_ATTACHMENT_EML, "message/rfc822", &config)
            .await
            .expect("parent email should survive an over-budget inline attachment");
        let plain = crate::rendering::render_plain(&doc);
        let children = doc.children.as_ref().expect("attachment child must be retained");

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "note.txt");
        assert_eq!(children[0].result.content.trim(), "Attachment body");
        assert!(plain.contains("Parent body"));
        assert!(!plain.contains("Attachment body"));
        assert_eq!(doc.processing_warnings.len(), 1);
        assert_eq!(doc.processing_warnings[0].source, "email_attachment_inlining");
        assert!(doc.processing_warnings[0].message.contains("note.txt"));
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_email_preserves_nested_message_child() {
        let config = ExtractionConfig {
            max_archive_depth: 1,
            ..Default::default()
        };

        let doc = EmailExtractor::new()
            .extract_content(NESTED_MESSAGE_EML, "message/rfc822", &config)
            .await
            .unwrap();
        let children = doc.children.expect("nested message should be extracted");

        assert_eq!(children.len(), 1, "nested message must be emitted exactly once");
        let nested = &children[0];
        assert_eq!(nested.path, "nested_message_0.eml");
        assert_eq!(nested.mime_type, "message/rfc822");
        assert!(nested.result.content.contains("Nested body"));
        assert!(!doc.elements.iter().any(|element| {
            matches!(element.kind, crate::types::internal::ElementKind::Heading { level: 2 })
                && element.text == nested.path
        }));
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_digest_emits_implicit_nested_message_once() {
        let config = ExtractionConfig {
            max_archive_depth: 1,
            ..Default::default()
        };
        let parsed = crate::extraction::email::parse_eml_content(IMPLICIT_NESTED_MESSAGE_EML).unwrap();

        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].mime_type.as_deref(), Some("message/rfc822"));

        let doc = EmailExtractor::new()
            .extract_content(IMPLICIT_NESTED_MESSAGE_EML, "message/rfc822", &config)
            .await
            .unwrap();
        let children = doc.children.expect("implicit nested message should be extracted");

        assert_eq!(
            children.len(),
            1,
            "implicit nested message must be emitted exactly once"
        );
        assert_eq!(children[0].path, "nested_message_0.eml");
        assert_eq!(children[0].mime_type, "message/rfc822");
        assert!(children[0].result.content.contains("Implicit nested body"));
    }

    #[test]
    fn test_sync_email_preserves_nested_attachment_payload() {
        let parsed = crate::extraction::email::parse_eml_content(NESTED_MESSAGE_EML).unwrap();

        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].mime_type.as_deref(), Some("message/rfc822"));
        let data = parsed.attachments[0]
            .data
            .as_deref()
            .expect("sync extraction must retain nested attachment bytes");
        assert!(String::from_utf8_lossy(data).contains("Nested body"));
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_email_skips_nested_payloads_at_depth_zero() {
        let config = ExtractionConfig {
            max_archive_depth: 0,
            ..Default::default()
        };

        let doc = EmailExtractor::new()
            .extract_content(NESTED_MESSAGE_EML, "message/rfc822", &config)
            .await
            .unwrap();

        assert!(doc.children.is_none(), "depth zero must not emit nested children");
        assert!(
            doc.processing_warnings.is_empty(),
            "depth zero is an intentional recursion limit"
        );
    }

    #[cfg(feature = "tokio-runtime")]
    #[tokio::test]
    async fn test_async_email_rejects_nested_payload_before_copying_over_cap() {
        let config = ExtractionConfig {
            max_archive_depth: 1,
            max_embedded_file_bytes: Some(8),
            ..Default::default()
        };
        let parsed = crate::extraction::email::extract_email_content_with_nested(
            NESTED_MESSAGE_EML,
            "message/rfc822",
            None,
            config.max_embedded_file_bytes,
            &crate::extractors::security::SecurityLimits::default(),
        )
        .unwrap();

        assert!(parsed.result.attachments[0].data.is_none());
        assert!(parsed.nested_messages[0].data.is_none());

        let doc = EmailExtractor::new()
            .extract_content(NESTED_MESSAGE_EML, "message/rfc822", &config)
            .await
            .unwrap();

        assert!(doc.children.is_none(), "over-cap nested messages must not be extracted");
        assert_eq!(doc.processing_warnings.len(), 1);
        assert_eq!(doc.processing_warnings[0].source, "nested_message_extraction");
        assert!(doc.processing_warnings[0].message.contains("exceeds cap 8 bytes"));
    }

    #[test]
    fn test_email_with_table_and_list_preservation() {
        let eml = r#"From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Table and List Repro
Content-Type: multipart/alternative; boundary="boundary"

--boundary
Content-Type: text/plain; charset=utf-8

Plain text fallback.

--boundary
Content-Type: text/html; charset=utf-8

<html>
<body>
    <p>Introduction.</p>
    <table>
        <tr><th>Name</th><th>Value</th></tr>
        <tr><td>Key</td><td>123</td></tr>
    </table>
    <ol>
        <li>First item</li>
        <li>Second item</li>
    </ol>
    <p>Conclusion.</p>
</body>
</html>
--boundary--
"#;
        let extractor = EmailExtractor::new();
        let config = ExtractionConfig::default();
        let doc = extractor
            .extract_sync(eml.as_bytes(), "message/rfc822", &config)
            .unwrap();

        let has_table = doc
            .elements
            .iter()
            .any(|e| matches!(e.kind, crate::types::internal::ElementKind::Table { .. }));
        let list_item_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::ListItem { .. }))
            .count();
        let paragraph_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Paragraph))
            .count();

        assert!(has_table, "Table element should be present");
        assert_eq!(list_item_count, 2, "Should have 2 list items");
        assert_eq!(
            paragraph_count, 2,
            "Should have exactly 2 body paragraphs (no duplicates)"
        );
    }

    #[test]
    fn test_content_size_guard_fires_when_limit_exceeded() {
        use crate::extractors::security::SecurityLimits;

        let long_body = "x".repeat(1000);
        let eml = format!("From: sender@example.com\r\nSubject: Big\r\n\r\n{}", long_body);

        let config = ExtractionConfig {
            security_limits: Some(SecurityLimits {
                max_content_size: 10,
                ..SecurityLimits::default()
            }),
            ..Default::default()
        };

        let extractor = EmailExtractor::new();
        let result = extractor.extract_sync(eml.as_bytes(), "message/rfc822", &config);

        assert!(
            result.is_err(),
            "extraction must fail when content exceeds max_content_size"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.to_lowercase().contains("security") || err_msg.to_lowercase().contains("content"),
            "error must mention security or content: {}",
            err_msg
        );
    }

    #[test]
    fn test_content_size_guard_passes_for_normal_email() {
        let eml = "From: sender@example.com\r\nSubject: Hi\r\n\r\nHello world.";
        let config = ExtractionConfig::default();
        let extractor = EmailExtractor::new();
        let result = extractor.extract_sync(eml.as_bytes(), "message/rfc822", &config);
        assert!(result.is_ok(), "normal email must not be rejected: {:?}", result.err());
    }
}
