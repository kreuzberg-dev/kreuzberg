//! Plain text extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::transform::normalize_line_endings;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use async_trait::async_trait;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const TEXT_WARNING_SOURCE: &str = "text";

/// Plain text extractor.
///
/// Extracts content from plain text files (.txt).
#[cfg_attr(alef, alef(skip))]
pub struct PlainTextExtractor;

impl PlainTextExtractor {
    /// Create a new plain text extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for PlainTextExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PlainTextExtractor {
    /// Build an `InternalDocument` from plain text content.
    ///
    /// Splits on double-newlines into paragraphs, after normalizing all line endings to
    /// `\n` so CRLF and lone-CR blank lines are recognized as paragraph boundaries the
    /// same way as bare LF ones.
    ///
    /// The normalization cannot be left to `extraction::transform`: populating an
    /// `InternalDocument` here makes `transform_extraction_result_to_elements` take the
    /// `convert_internal_elements_to_elements` path, which never reaches that module's
    /// paragraph splitter. Without this, `\r\n\r\n` and `\r\r` never match the `"\n\n"`
    /// boundary and the whole document collapses into one paragraph (#227).
    fn build_internal_document(text: &str) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("text");
        let normalized = normalize_line_endings(text);
        for paragraph in normalized.split("\n\n") {
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                builder.push_paragraph(trimmed, vec![], None, None);
            }
        }
        builder.build()
    }
}

impl Plugin for PlainTextExtractor {
    fn name(&self) -> &str {
        "plain-text-extractor"
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
        "Extracts content from plain text files"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for PlainTextExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, _config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        _config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        // Both builds must answer "was any content actually lost?" the same way (#171,
        // #395). Inspecting the decoded string for U+FFFD only works without `quality`:
        // there `String::from_utf8_lossy` substitutes one per undecodable byte, and the
        // extra `from_utf8` check keeps a document that legitimately *contains* U+FFFD
        // from being reported as damaged. Under `quality` that check is structurally
        // blind, because `safe_decode`'s mojibake cleanup strips every replacement
        // character before returning. So take the answer from the decoder itself, which
        // records it at the point of loss.
        //
        // Note the deliberate asymmetry with `fell_back`: reinterpreting bytes under a
        // detected single-byte encoding is not data loss (windows-1252 and iso-8859-1
        // map every byte 0x00-0xFF and can never fail), so only `replaced_characters`
        // warrants a warning. ~keep
        let outcome = crate::utils::decode_with_provenance(content, None);
        let decoded_lossily = outcome.replaced_characters;
        let decoded = outcome.text;

        let text = crate::utils::strip_bom(&decoded)
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        let line_count = text.lines().count();
        let word_count = text.split_whitespace().count();
        let character_count = text.chars().count();

        let mut doc = Self::build_internal_document(&text);
        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                TEXT_WARNING_SOURCE,
                "text source",
            );
        }

        doc.metadata = Metadata {
            format: Some(crate::types::FormatMetadata::Text(crate::types::TextMetadata {
                line_count: line_count as u32,
                word_count: word_count as u32,
                character_count: character_count as u32,
                headers: None,
                links: None,
                code_blocks: None,
            })),
            ..Default::default()
        };
        doc.mime_type = mime_type.to_string();

        Ok(doc)
    }

    /// Only `text/plain`.
    ///
    /// This extractor used to also claim `text/asciidoc`, `text/x-asciidoc`, `text/vtt`,
    /// `text/troff`, `text/x-mdoc`, `text/x-pod` and `text/x-dokuwiki` while doing nothing
    /// beyond a BOM strip and a blank-line paragraph split, so every one of those formats
    /// silently produced structureless garbage that looked like a successful extraction
    /// (#228).
    ///
    /// AsciiDoc and WebVTT now have real extractors
    /// ([`AsciiDocExtractor`](crate::extractors::AsciiDocExtractor),
    /// [`WebVttExtractor`](crate::extractors::WebVttExtractor)). troff, mdoc, POD and
    /// DokuWiki were dropped from the format catalogue entirely rather than advertised
    /// with no implementation behind them.
    fn supported_mime_types(&self) -> &[&str] {
        &["text/plain"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plain_text_extractor() {
        let extractor = PlainTextExtractor::new();
        let content = b"Hello, World!\nThis is a test.";
        let config = ExtractionConfig::default();

        let result = extractor.extract_content(content, "text/plain", &config).await.unwrap();

        assert!(result.metadata.format.is_some());
        let text_meta = match result.metadata.format.as_ref().unwrap() {
            crate::types::FormatMetadata::Text(meta) => meta,
            _ => panic!("Expected Text metadata"),
        };
        assert_eq!(text_meta.line_count, 2);
        assert_eq!(text_meta.word_count, 6);
    }

    fn text_warnings(doc: &crate::types::internal::InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == TEXT_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: invalid UTF-8 bytes are decoded lossily, which without `quality` is a
    /// deterministic property of `String::from_utf8_lossy` -- every undecodable byte
    /// is replaced, so `decode_with_provenance` reports `replaced_characters`.
    ///
    /// Deliberately not run under `quality` (#395): there chardetng resolves these
    /// bytes to a single-byte encoding that maps all of 0x00-0xFF, so nothing is
    /// *replaced* -- the bytes are reinterpreted. That is `fell_back`, not data loss,
    /// and reporting no warning for it is now correct rather than a blind spot.
    #[cfg(not(feature = "quality"))]
    #[tokio::test]
    async fn should_warn_when_text_source_is_not_valid_utf8() {
        let extractor = PlainTextExtractor::new();
        let config = ExtractionConfig::default();
        let content: &[u8] = &[b'A', 0xFF, 0xFE, b'B'];

        let result = extractor
            .extract_content(content, "text/plain", &config)
            .await
            .expect("extraction of invalid UTF-8 must still succeed");

        let warnings = text_warnings(&result);
        assert_eq!(warnings.len(), 1, "expected exactly one text warning, got {warnings:?}");
        assert!(
            warnings[0].contains("not valid UTF-8") && warnings[0].contains("replacement character"),
            "warning must describe the lossy decode, got {warnings:?}"
        );
    }

    /// A valid UTF-8 document -- including one that happens to contain a literal
    /// U+FFFD character of its own -- must not warn.
    #[tokio::test]
    async fn valid_utf8_text_produces_zero_warnings() {
        let extractor = PlainTextExtractor::new();
        let config = ExtractionConfig::default();
        let content = "Hello, World!\nThis contains a literal \u{FFFD} character.".as_bytes();

        let result = extractor
            .extract_content(content, "text/plain", &config)
            .await
            .expect("extraction should succeed");

        assert!(
            text_warnings(&result).is_empty(),
            "valid UTF-8 must not warn even with a literal U+FFFD, got {:?}",
            text_warnings(&result)
        );
    }

    #[test]
    fn test_plain_text_plugin_interface() {
        let extractor = PlainTextExtractor::new();
        assert_eq!(extractor.name(), "plain-text-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.supported_mime_types(), &["text/plain"]);
        assert_eq!(extractor.priority(), 50);
    }
}
