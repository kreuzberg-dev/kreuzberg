//! Apple Pages (.pages) extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::iwork::{
    IwaExpansionBudget, dedup_text, extract_metadata_from_zip, extract_text_from_proto, push_member_parse_warning,
    read_iwa_file, validate_iwork_zip,
};
use crate::extractors::security::{SecurityBudget, SecurityLimits};
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ProcessingWarning;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use async_trait::async_trait;

/// Apple Pages document extractor.
///
/// Supports `.pages` files (modern iWork format, 2013+).
///
/// Extracts all text content from the document by parsing the IWA
/// (iWork Archive) container: ZIP → Snappy → protobuf text fields.
#[cfg_attr(alef, alef(skip))]
pub struct PagesExtractor;

impl PagesExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for PagesExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for PagesExtractor {
    fn name(&self) -> &str {
        "iwork-pages-extractor"
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
        "Apple Pages (.pages) text extraction via IWA container parser"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Parsed Pages data: document text blocks and metadata.
struct PagesData {
    /// Text blocks from the main document IWA files (prioritized).
    document_texts: Vec<String>,
    /// Additional text from annotation/data-record IWA files.
    supplementary_texts: Vec<String>,
    /// Metadata extracted from the ZIP archive.
    metadata: crate::types::metadata::Metadata,
    /// Warnings for IWA members that failed to parse (#106).
    warnings: Vec<ProcessingWarning>,
}

/// Parse a Pages ZIP and extract all text from IWA files.
///
/// Pages stores its content in:
/// - `Index/Document.iwa` — main document text
/// - `Index/AnnotationAuthorStorage.iwa` — comments/annotations
/// - Any `DataRecords/*.iwa` — embedded data blocks
///
/// We prioritize Document IWA files for the main body and separate
/// annotation/data content.
fn parse_pages(content: &[u8], limits: &SecurityLimits) -> Result<PagesData> {
    validate_iwork_zip(content, limits)?;
    let mut budget = SecurityBudget::for_iwork(limits);
    let mut expansion = IwaExpansionBudget::from_limits(limits);
    let iwa_paths = super::collect_iwa_paths(content)?;
    let metadata = extract_metadata_from_zip(content);

    let mut doc_paths: Vec<&String> = Vec::new();
    let mut other_paths: Vec<&String> = Vec::new();

    for path in &iwa_paths {
        let filename = path.rsplit('/').next().unwrap_or(path);
        if filename.starts_with("Document") || filename.starts_with("Section") || filename.starts_with("Text") {
            doc_paths.push(path);
        } else {
            other_paths.push(path);
        }
    }

    if doc_paths.is_empty() {
        doc_paths = iwa_paths.iter().collect();
        other_paths.clear();
    }

    let mut warnings: Vec<ProcessingWarning> = Vec::new();

    let mut doc_texts: Vec<String> = Vec::new();
    for path in &doc_paths {
        match read_iwa_file(content, path, &mut expansion) {
            Ok(decompressed) => {
                let texts = extract_text_from_proto(&decompressed, &mut budget)?;
                doc_texts.extend(texts);
            }
            Err(error) if matches!(&error, crate::error::XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(%error, "Skipping IWA file (decompression failed): {path}");
                push_member_parse_warning(&mut warnings, path, &error);
            }
        }
    }

    let mut other_texts_raw: Vec<String> = Vec::new();
    for path in &other_paths {
        match read_iwa_file(content, path, &mut expansion) {
            Ok(decompressed) => {
                let texts = extract_text_from_proto(&decompressed, &mut budget)?;
                other_texts_raw.extend(texts);
            }
            Err(error) if matches!(&error, crate::error::XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(%error, "Skipping IWA file (decompression failed): {path}");
                push_member_parse_warning(&mut warnings, path, &error);
            }
        }
    }

    let document_texts = dedup_text(doc_texts);
    let supplementary_texts: Vec<String> = dedup_text(other_texts_raw)
        .into_iter()
        .filter(|t| !document_texts.contains(t))
        .collect();

    Ok(PagesData {
        document_texts,
        supplementary_texts,
        metadata,
        warnings,
    })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PagesExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let data = {
            #[cfg(feature = "tokio-runtime")]
            if crate::core::batch_mode::is_batch_mode() {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let content_owned = content.to_vec();
                let limits = config.security_limits.clone().unwrap_or_default();
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || {
                    let _guard = span.entered();
                    parse_pages(&content_owned, &limits)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("Pages extraction task failed: {e}")))??
            } else {
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_pages(content, &limits)?
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_pages(content, &limits)?
            }
        };

        let mut doc = build_pages_internal_document(&data);
        doc.mime_type = mime_type.to_string();
        for warning in data.warnings {
            crate::core::diagnostics::push_warning_deduped(&mut doc.processing_warnings, warning);
        }
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-iwork-pages-sffpages"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Build an `InternalDocument` from parsed Pages data.
///
/// Applies heading detection heuristics: short lines (under 80 chars) that
/// appear before longer text blocks are treated as headings. Metadata from the
/// ZIP archive is applied to the document.
fn build_pages_internal_document(data: &PagesData) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("pages");

    if data.metadata.title.is_some() || data.metadata.authors.is_some() {
        builder.set_metadata(data.metadata.clone());
    }

    let texts = &data.document_texts;
    let mut start_idx = 0;
    if let Some(first) = texts.first() {
        let trimmed = first.trim();
        if !trimmed.is_empty() && is_likely_title(trimmed) && texts.len() > 1 {
            builder.push_title(trimmed, None, None);
            start_idx = 1;
        }
    }

    for text in &texts[start_idx..] {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_likely_heading(trimmed) {
            builder.push_heading(2, trimmed, None, None);
        } else {
            builder.push_paragraph(trimmed, vec![], None, None);
        }
    }

    if !data.supplementary_texts.is_empty() {
        let has_body = !data.document_texts.is_empty();
        if has_body {
            builder.push_heading(2, "Annotations", None, None);
        }
        for text in &data.supplementary_texts {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                builder.push_paragraph(trimmed, vec![], None, None);
            }
        }
    }

    builder.build()
}

/// Heuristic: a string looks like a document title if it is short, does not
/// end with sentence-terminating punctuation, and contains at least one
/// alphabetic character.
fn is_likely_title(s: &str) -> bool {
    s.len() <= 100
        && !s.ends_with('.')
        && !s.ends_with('!')
        && !s.ends_with('?')
        && s.chars().any(|c| c.is_alphabetic())
        && !s.contains('\n')
}

/// Heuristic: a string looks like a heading if it is relatively short, does
/// not end with sentence-terminating punctuation, and starts with an uppercase
/// letter or a digit.
fn is_likely_heading(s: &str) -> bool {
    s.len() <= 80
        && !s.ends_with('.')
        && !s.ends_with(',')
        && !s.contains('\n')
        && s.chars().next().is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
        && s.split_whitespace().count() <= 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pages_extractor_plugin_interface() {
        let extractor = PagesExtractor::new();
        assert_eq!(extractor.name(), "iwork-pages-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_pages_extractor_supported_mime_types() {
        let extractor = PagesExtractor::new();
        let types = extractor.supported_mime_types();
        assert!(types.contains(&"application/x-iwork-pages-sffpages"));
    }

    fn iwa_text_frame(text: &str) -> Vec<u8> {
        let mut payload = vec![0x1A, text.len() as u8];
        payload.extend_from_slice(text.as_bytes());
        let mut frame = vec![1, 0, 0, 0];
        let length = payload.len();
        frame[1] = (length & 0xff) as u8;
        frame[2] = ((length >> 8) & 0xff) as u8;
        frame[3] = ((length >> 16) & 0xff) as u8;
        frame.extend_from_slice(&payload);
        frame
    }

    fn pages_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                zip.start_file(*name, options).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    /// Regression for #106: a Document IWA member that fails to decompress
    /// must surface a named `ProcessingWarning`, not vanish silently.
    #[test]
    fn should_warn_when_a_document_iwa_member_fails_to_parse() {
        let good = iwa_text_frame("Body text");
        let broken: Vec<u8> = vec![1, 0, 0];
        let archive = pages_zip(&[("Index/Document-1.iwa", &good), ("Index/Document-2.iwa", &broken)]);

        let data = parse_pages(&archive, &SecurityLimits::default()).unwrap();

        assert_eq!(data.document_texts, vec!["Body text".to_string()]);
        assert_eq!(data.warnings.len(), 1);
        assert_eq!(data.warnings[0].source, "iwork");
        assert!(
            data.warnings[0].message.contains("Index/Document-2.iwa"),
            "warning must name the failed member: {}",
            data.warnings[0].message
        );
    }
}
