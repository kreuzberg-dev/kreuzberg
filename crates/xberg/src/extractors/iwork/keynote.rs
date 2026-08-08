//! Apple Keynote (.key) extractor.

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

/// Apple Keynote presentation extractor.
///
/// Supports `.key` files (modern iWork format, 2013+).
///
/// Extracts slide text and speaker notes from the IWA container:
/// ZIP → Snappy → protobuf text fields.
#[cfg_attr(alef, alef(skip))]
pub struct KeynoteExtractor;

impl KeynoteExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for KeynoteExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for KeynoteExtractor {
    fn name(&self) -> &str {
        "iwork-keynote-extractor"
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
        "Apple Keynote (.key) text extraction via IWA container parser"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Parsed Keynote data: per-slide text and metadata.
struct KeynoteData {
    /// Text extracted from individual slide IWA files, in path-sorted order.
    slide_texts: Vec<Vec<String>>,
    /// Additional text from non-slide IWA files (notes, master slides, etc.).
    other_texts: Vec<String>,
    /// Metadata extracted from the ZIP archive.
    metadata: crate::types::metadata::Metadata,
    /// Warnings for IWA members that failed to parse (#106).
    warnings: Vec<ProcessingWarning>,
}

/// Parse a Keynote ZIP and extract all text from IWA files.
///
/// Keynote stores its content across many IWA files:
/// - `Index/Presentation.iwa` — master slide structure and layout
/// - `Index/Slide-*.iwa` / `Index/Slide_*.iwa` — individual slide content and speaker notes
/// - `Index/MasterSlide-*.iwa` / `Index/MasterSlide_*.iwa` — master slide text
///
/// We separate slide-specific IWA files from other files to produce
/// per-slide structured output.
fn parse_keynote(content: &[u8], limits: &SecurityLimits) -> Result<KeynoteData> {
    validate_iwork_zip(content, limits)?;
    let mut budget = SecurityBudget::for_iwork(limits);
    let mut expansion = IwaExpansionBudget::from_limits(limits);
    let iwa_paths = super::collect_iwa_paths(content)?;
    let metadata = extract_metadata_from_zip(content);

    let mut slide_paths: Vec<&String> = iwa_paths
        .iter()
        .filter(|p| {
            let filename = p.rsplit('/').next().unwrap_or(p);
            filename.starts_with("Slide") && !filename.starts_with("MasterSlide")
        })
        .collect();

    slide_paths.sort();

    let other_paths: Vec<&String> = iwa_paths
        .iter()
        .filter(|p| {
            let filename = p.rsplit('/').next().unwrap_or(p);
            !filename.starts_with("Slide") || filename.starts_with("MasterSlide")
        })
        .collect();

    let mut warnings: Vec<ProcessingWarning> = Vec::new();
    let mut slide_texts: Vec<Vec<String>> = Vec::new();

    // Each slide keeps its own text, deduped only within itself (#101): a
    // footer or title legitimately repeated across slides must survive on
    // every slide it appears on, not just the first.
    for path in &slide_paths {
        match read_iwa_file(content, path, &mut expansion) {
            Ok(decompressed) => {
                let texts = extract_text_from_proto(&decompressed, &mut budget)?;
                let deduped = dedup_text(texts);
                if !deduped.is_empty() {
                    slide_texts.push(deduped);
                }
            }
            Err(error) if matches!(&error, crate::error::XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(%error, "Skipping IWA file (decompression failed): {path}");
                push_member_parse_warning(&mut warnings, path, &error);
            }
        }
    }

    // Text already shown on a slide is only worth repeating in "Additional
    // Content" if it says something new; text unique to a slide's own repeats
    // was already preserved above.
    let mut seen_in_slides: std::collections::HashSet<String> = slide_texts.iter().flatten().cloned().collect();

    let mut other_raw: Vec<String> = Vec::new();
    for path in &other_paths {
        match read_iwa_file(content, path, &mut expansion) {
            Ok(decompressed) => {
                let texts = extract_text_from_proto(&decompressed, &mut budget)?;
                other_raw.extend(texts);
            }
            Err(error) if matches!(&error, crate::error::XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(%error, "Skipping IWA file (decompression failed): {path}");
                push_member_parse_warning(&mut warnings, path, &error);
            }
        }
    }

    let other_texts: Vec<String> = dedup_text(other_raw)
        .into_iter()
        .filter(|t| seen_in_slides.insert(t.clone()))
        .collect();

    Ok(KeynoteData {
        slide_texts,
        other_texts,
        metadata,
        warnings,
    })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for KeynoteExtractor {
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
                    parse_keynote(&content_owned, &limits)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("Keynote extraction task failed: {e}")))??
            } else {
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_keynote(content, &limits)?
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_keynote(content, &limits)?
            }
        };

        let mut doc = build_keynote_internal_document(&data);
        doc.mime_type = mime_type.to_string();
        for warning in data.warnings {
            crate::core::diagnostics::push_warning_deduped(&mut doc.processing_warnings, warning);
        }
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-iwork-keynote-sffkey"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Build an `InternalDocument` from parsed Keynote data.
///
/// Creates a slide element for each detected slide group, using the first text
/// line as the slide title. Additional lines become paragraphs.
/// Metadata from the ZIP archive is applied to the document.
fn build_keynote_internal_document(data: &KeynoteData) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("keynote");

    if data.metadata.title.is_some() || data.metadata.authors.is_some() {
        builder.set_metadata(data.metadata.clone());
    }

    for (index, slide_lines) in data.slide_texts.iter().enumerate() {
        if slide_lines.is_empty() {
            continue;
        }

        let title = slide_lines[0].trim();
        builder.push_slide((index + 1) as u32, Some(title), None);

        for line in &slide_lines[1..] {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                builder.push_paragraph(trimmed, vec![], None, None);
            }
        }
    }

    if !data.other_texts.is_empty() {
        let has_slides = !data.slide_texts.is_empty();
        if has_slides {
            builder.push_heading(2, "Additional Content", None, None);
        }
        for text in &data.other_texts {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                builder.push_paragraph(trimmed, vec![], None, None);
            }
        }
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal::ElementKind;

    #[test]
    fn test_keynote_extractor_plugin_interface() {
        let extractor = KeynoteExtractor::new();
        assert_eq!(extractor.name(), "iwork-keynote-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_keynote_extractor_supported_mime_types() {
        let extractor = KeynoteExtractor::new();
        let types = extractor.supported_mime_types();
        assert!(types.contains(&"application/x-iwork-keynote-sffkey"));
    }

    /// Build an uncompressed (chunk type 0x01) IWA byte stream wrapping a
    /// single length-delimited protobuf text field.
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

    fn keynote_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
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

    /// Regression for #101: the same repeated text ("Confidential") on two
    /// different slides must appear on *both* slides, not just the first.
    #[test]
    fn should_keep_text_repeated_across_different_slides() {
        let slide1 = iwa_text_frame("Confidential");
        let slide2 = iwa_text_frame("Confidential");
        let archive = keynote_zip(&[("Index/Slide-1.iwa", &slide1), ("Index/Slide-2.iwa", &slide2)]);

        let data = parse_keynote(&archive, &SecurityLimits::default()).unwrap();

        assert_eq!(
            data.slide_texts.len(),
            2,
            "both slides must be kept: {:?}",
            data.slide_texts
        );
        assert_eq!(data.slide_texts[0], vec!["Confidential".to_string()]);
        assert_eq!(
            data.slide_texts[1],
            vec!["Confidential".to_string()],
            "repeated content on the second slide must not be dropped"
        );
    }

    /// Regression for #106: a member that fails to decompress must surface a
    /// named `ProcessingWarning`, not vanish silently.
    #[test]
    fn should_warn_when_an_iwa_member_fails_to_parse() {
        let good_slide = iwa_text_frame("Body");
        // Malformed IWA framing: a chunk type byte with no length/payload.
        let broken_slide: Vec<u8> = vec![1, 0, 0];
        let archive = keynote_zip(&[("Index/Slide-1.iwa", &good_slide), ("Index/Slide-2.iwa", &broken_slide)]);

        let data = parse_keynote(&archive, &SecurityLimits::default()).unwrap();

        assert_eq!(data.slide_texts, vec![vec!["Body".to_string()]]);
        assert_eq!(data.warnings.len(), 1, "the broken member must be named in a warning");
        assert_eq!(data.warnings[0].source, "iwork");
        assert!(
            data.warnings[0].message.contains("Index/Slide-2.iwa"),
            "warning must name the failed member: {}",
            data.warnings[0].message
        );
    }

    #[test]
    fn should_preserve_slide_element_semantics() {
        let data = KeynoteData {
            slide_texts: vec![vec!["Title".to_string(), "Body".to_string()]],
            other_texts: Vec::new(),
            metadata: crate::types::metadata::Metadata::default(),
            warnings: Vec::new(),
        };

        let document = build_keynote_internal_document(&data);

        assert_eq!(document.elements[0].kind, ElementKind::Slide { number: 1 });
        assert_eq!(document.elements[0].text, "Title");
        assert_eq!(document.elements[1].kind, ElementKind::Paragraph);
        assert_eq!(document.elements[1].text, "Body");
    }
}
