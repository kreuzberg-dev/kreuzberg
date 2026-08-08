//! Built-in Middle-stage post-processor that decodes QR codes inside every
//! [`ExtractedImage`](crate::types::ExtractedImage).
//!
//! Activates when [`ExtractionConfig::qr_codes`](crate::core::config::ExtractionConfig::qr_codes)
//! is `Some(true)`. Walks `result.images.iter_mut()`, runs the pure-Rust
//! [`crate::extractors::qr::detect_qr_codes`] decoder on each image's bytes,
//! and writes the result into [`crate::types::ExtractedImage::qr_codes`]. A `Some(vec![])`
//! is written when detection ran but found nothing; `None` only when QR
//! detection was not enabled.
//!
//! The decoded payloads are then appended as a "QR Codes" section to the
//! document text so that every downstream enrichment stage — chunking,
//! embeddings, keywords, language detection — sees them as ordinary text. The
//! section is appended to **both** `content` and `formatted_content`, because
//! `apply_output_format` replaces `content` with `formatted_content` after this
//! stage runs: appending to `content` alone is silently discarded for every
//! non-`Plain` output format (#302).
//!
//! URL-shaped payloads are additionally routed into the document's existing URI
//! collection (`ExtractedDocument::uris`), the same list hyperlinks land in, so
//! consumers have one place to look for links rather than two.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::core::config::{ExtractionConfig, OutputFormat};
use crate::core::diagnostics::push_warning;
use crate::extractors::qr::detect_qr_codes;
use crate::plugins::{Plugin, PostProcessor, ProcessingStage, register_post_processor};
use crate::types::internal::InternalDocument;
use crate::types::uri::classify_uri;
use crate::types::{ExtractedDocument, ExtractedUri};

/// `ProcessingWarning::source` used for every warning this processor raises.
const WARNING_SOURCE: &str = "qr-codes";

/// URI schemes a QR payload must start with to be treated as a link. QR codes
/// also carry non-URI payloads (plain text, `WIFI:`/`BEGIN:VCARD` blobs); those
/// stay text-only rather than polluting the URI list.
const URI_SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

/// Post-processor that runs `rqrr` over each image, writes the decoded QR
/// payloads into [`crate::types::ExtractedImage::qr_codes`], and surfaces them
/// in the document text (and, for URL payloads, in
/// [`ExtractedDocument::uris`]) so downstream enrichment sees them.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, Default)]
pub struct QrCodeProcessor;

impl Plugin for QrCodeProcessor {
    fn name(&self) -> &str {
        "qr-codes"
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
impl PostProcessor for QrCodeProcessor {
    async fn process(&self, result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
        if config.qr_codes != Some(true) {
            return Ok(());
        }
        let Some(images) = result.images.as_mut() else {
            return Ok(());
        };
        if images.is_empty() {
            return Ok(());
        }

        tracing::info!(
            target: "xberg::qr_codes",
            images = images.len(),
            "running rqrr QR detection across extracted images"
        );

        let mut payloads: Vec<String> = Vec::new();
        let mut lossy_image_indices: Vec<usize> = Vec::new();
        for (index, image) in images.iter_mut().enumerate() {
            let codes = detect_qr_codes(image.data.as_ref(), Some(image.format.as_ref()));
            for code in &codes {
                // A grid that decodes to bytes that are not valid UTF-8 is kept (partial
                // result) with the undecodable bytes replaced, but the caller is told.
                if code.payload.contains(char::REPLACEMENT_CHARACTER) {
                    lossy_image_indices.push(index);
                }
                payloads.push(code.payload.clone());
            }
            image.qr_codes = Some(codes);
        }

        for index in lossy_image_indices {
            push_warning(
                &mut result.processing_warnings,
                WARNING_SOURCE,
                format!(
                    "A QR code in image {index} decoded to bytes that are not valid UTF-8; the \
                     undecodable bytes were replaced with U+FFFD and the payload is included as-is"
                ),
            );
        }

        if payloads.is_empty() {
            return Ok(());
        }

        // Decoded payloads previously only landed on `ExtractedImage::qr_codes`, which nothing
        // in `rendering/` reads — the text never reached `content` (xberg-io/xberg#156). They
        // are appended as their own section so the text participates in every downstream stage
        // that reads document text, chunking and embeddings included (#302).
        append_payload_section(result, config, &payloads);
        collect_payload_uris(result, &payloads);

        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }

    fn should_process(&self, _result: &ExtractedDocument, config: &ExtractionConfig) -> bool {
        config.qr_codes == Some(true)
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Append the decoded payloads to the document text.
///
/// `content` always receives the section. `formatted_content` receives it too
/// whenever the requested output format is one this processor can extend
/// safely — that matters because `apply_output_format` overwrites `content`
/// with `formatted_content` after this stage, and the final chunking pass reads
/// `formatted_content` for every non-`Plain` format. `Json` and `Custom(_)`
/// renderings are emitted verbatim by a renderer that owns their syntax, so a
/// text section cannot be spliced into them; those get a warning instead of a
/// corrupted document.
fn append_payload_section(result: &mut ExtractedDocument, config: &ExtractionConfig, payloads: &[String]) {
    append_section(&mut result.content, &markdown_section(payloads));

    let renders_as_text = match config.output_format {
        OutputFormat::Plain
        | OutputFormat::Markdown
        | OutputFormat::Djot
        | OutputFormat::Html
        | OutputFormat::Structured => true,
        OutputFormat::Json | OutputFormat::Custom(_) => false,
    };

    if renders_as_text {
        if let Some(formatted) = result.formatted_content.as_mut() {
            let section = if config.output_format == OutputFormat::Html {
                html_section(payloads)
            } else {
                markdown_section(payloads)
            };
            append_section(formatted, &section);
        }
        return;
    }

    if result.formatted_content.is_some() {
        push_warning(
            &mut result.processing_warnings,
            WARNING_SOURCE,
            format!(
                "{} decoded QR payload(s) could not be merged into the requested output format \
                 because it is produced verbatim by a renderer; the payloads are available on \
                 the per-image `qr_codes` field but are absent from the returned content, \
                 chunks and embeddings",
                payloads.len()
            ),
        );
    }
}

/// Route URL-shaped payloads into the document's existing URI list.
///
/// Uses the same de-duplication key as `derive_extraction_result` (`url` +
/// `kind`) and the same per-document cap as `InternalDocument::push_uri`, so a
/// QR payload is indistinguishable from a hyperlink found anywhere else and a
/// pathological document cannot grow the list without bound.
fn collect_payload_uris(result: &mut ExtractedDocument, payloads: &[String]) {
    if !payloads.iter().any(|payload| is_uri_payload(payload)) {
        return;
    }

    let uris = result.uris.get_or_insert_with(Vec::new);
    let mut dropped = 0usize;
    for payload in payloads.iter().filter(|payload| is_uri_payload(payload)) {
        let kind = classify_uri(payload);
        if uris.iter().any(|uri| uri.url == *payload && uri.kind == kind) {
            continue;
        }
        if uris.len() >= InternalDocument::MAX_URIS {
            dropped += 1;
            continue;
        }
        uris.push(ExtractedUri {
            url: payload.clone(),
            label: None,
            page: None,
            kind,
        });
    }

    if dropped > 0 {
        push_warning(
            &mut result.processing_warnings,
            "uris",
            format!(
                "{dropped} QR payload URI(s) were dropped at the per-document limit of {} and \
                 are missing from the result",
                InternalDocument::MAX_URIS
            ),
        );
    }
}

/// Does `payload` look like a link rather than arbitrary QR text?
fn is_uri_payload(payload: &str) -> bool {
    URI_SCHEMES.iter().any(|scheme| {
        payload.len() > scheme.len()
            && payload
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    })
}

/// Render the payloads as a markdown/plain-text section (trailing newline, no
/// leading blank line — `append_section` owns the separation).
fn markdown_section(payloads: &[String]) -> String {
    let mut section = String::from("## QR Codes\n\n");
    for payload in payloads {
        section.push_str("- ");
        section.push_str(payload);
        section.push('\n');
    }
    section
}

/// Render the payloads as an HTML section for HTML output.
fn html_section(payloads: &[String]) -> String {
    let mut section = String::from("<h2>QR Codes</h2>\n<ul>\n");
    for payload in payloads {
        section.push_str("<li>");
        section.push_str(&escape_html(payload));
        section.push_str("</li>\n");
    }
    section.push_str("</ul>\n");
    section
}

/// Escape the three characters that would otherwise break out of HTML text.
fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Append `section` to `target`, separated by exactly one blank line, without
/// introducing leading blank lines when `target` is empty (a document whose
/// only text is a QR payload must not start with whitespace).
fn append_section(target: &mut String, section: &str) {
    if !target.is_empty() {
        if !target.ends_with('\n') {
            target.push('\n');
        }
        if !target.ends_with("\n\n") {
            target.push('\n');
        }
    }
    target.push_str(section);
}

/// Register the default QR post-processor with the global registry.
#[cfg_attr(alef, alef(skip))]
pub fn register() -> Result<()> {
    register_post_processor(Arc::new(QrCodeProcessor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExtractedImage;
    use bytes::Bytes;
    use std::borrow::Cow;

    fn empty_result_with_image() -> ExtractedDocument {
        ExtractedDocument {
            content: String::new(),
            mime_type: Cow::Borrowed("text/plain"),
            images: Some(vec![ExtractedImage {
                data: Bytes::from_static(&[]),
                format: Cow::Borrowed("png"),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn processor_metadata_is_correct() {
        let p = QrCodeProcessor;
        assert_eq!(p.name(), "qr-codes");
        assert_eq!(p.processing_stage(), ProcessingStage::Middle);
    }

    #[test]
    fn should_process_only_when_enabled() {
        let p = QrCodeProcessor;
        let result = empty_result_with_image();

        assert!(!p.should_process(&result, &ExtractionConfig::default()));

        let cfg = ExtractionConfig {
            qr_codes: Some(false),
            ..Default::default()
        };
        assert!(!p.should_process(&result, &cfg));

        let cfg = ExtractionConfig {
            qr_codes: Some(true),
            ..Default::default()
        };
        assert!(p.should_process(&result, &cfg));
    }

    #[tokio::test]
    async fn empty_image_bytes_yield_empty_vec() {
        let p = QrCodeProcessor;
        let mut result = empty_result_with_image();
        let cfg = ExtractionConfig {
            qr_codes: Some(true),
            ..Default::default()
        };
        p.process(&mut result, &cfg).await.unwrap();
        let images = result.images.as_ref().expect("images should still be Some");
        assert_eq!(images[0].qr_codes.as_deref(), Some(&[][..]));
    }

    #[tokio::test]
    async fn disabled_config_leaves_field_none() {
        let p = QrCodeProcessor;
        let mut result = empty_result_with_image();
        let cfg = ExtractionConfig::default();
        p.process(&mut result, &cfg).await.unwrap();
        let images = result.images.as_ref().expect("images should still be Some");
        assert!(images[0].qr_codes.is_none());
    }
}
