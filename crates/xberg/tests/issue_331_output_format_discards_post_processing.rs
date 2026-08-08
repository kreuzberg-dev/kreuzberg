//! Regression test for #331: a non-`Plain` output format must not discard post-processing.
//!
//! `derive_extraction_result` renders `formatted_content` from the extractor's element tree
//! at derive time — before any post-processor runs. `apply_output_format` then does
//! `result.content = formatted_content.take()` at the *end* of `run_pipeline`, after the
//! Early/Middle/Late post-processor stages. So for every output format that produces a
//! `formatted_content` (Markdown, Djot, Html, Json, Custom), each post-processor's rewrite
//! of `content` was overwritten by text rendered before that processor ever ran.
//!
//! The security-relevant instance: with the redaction post-processor configured and
//! `OutputFormat::Markdown`, the returned `content` was the *unredacted* markdown.

use async_trait::async_trait;
use xberg::plugins::Plugin;
use xberg::{
    ExtractInput, ExtractedDocument, ExtractionConfig, OutputFormat, PostProcessor, ProcessingStage, Result, extract,
    register_post_processor, unregister_post_processor,
};

const SECRET: &str = "ACCOUNT-1234";
const REDACTED: &str = "[REDACTED]";
const SOURCE_TEXT: &str = "Invoice for ACCOUNT-1234 is overdue.";
const PROCESSOR_NAME: &str = "issue-331-redactor";

/// Late-stage processor that rewrites `content`, exactly as the built-in redaction
/// processor does. A no-op on documents that do not carry [`SECRET`].
struct RedactingProcessor;

impl Plugin for RedactingProcessor {
    fn name(&self) -> &str {
        PROCESSOR_NAME
    }
}

#[async_trait]
impl PostProcessor for RedactingProcessor {
    async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
        result.content = result.content.replace(SECRET, REDACTED);
        if let Some(pages) = result.pages.as_mut() {
            for page in pages.iter_mut() {
                page.content = page.content.replace(SECRET, REDACTED);
            }
        }
        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Late
    }
}

async fn extract_with_redaction(output_format: OutputFormat) -> ExtractedDocument {
    register_post_processor(std::sync::Arc::new(RedactingProcessor)).expect("registering the processor should succeed");

    let config = ExtractionConfig {
        output_format,
        use_cache: false,
        ..Default::default()
    };
    let outcome = extract(
        ExtractInput::from_bytes(SOURCE_TEXT.as_bytes(), "text/plain", None),
        &config,
    )
    .await;

    unregister_post_processor(PROCESSOR_NAME).expect("unregistering the processor should succeed");

    outcome
        .expect("extraction should succeed")
        .results
        .into_iter()
        .next()
        .expect("one input yields one result")
}

/// The defect: Markdown output returned the pre-redaction text.
#[tokio::test]
async fn markdown_output_carries_post_processed_content() {
    let result = extract_with_redaction(OutputFormat::Markdown).await;

    assert!(
        !result.content.contains(SECRET),
        "markdown output leaked pre-post-processing text: {:?}",
        result.content
    );
    assert!(
        result.content.contains(REDACTED),
        "markdown output is missing the post-processor's replacement: {:?}",
        result.content
    );
}

/// Djot renders through the same `formatted_content` path.
#[tokio::test]
async fn djot_output_carries_post_processed_content() {
    let result = extract_with_redaction(OutputFormat::Djot).await;

    assert!(
        !result.content.contains(SECRET),
        "djot output leaked pre-post-processing text: {:?}",
        result.content
    );
}

/// The control: `Plain` never produced a `formatted_content`, so it was always correct and
/// must stay correct.
#[tokio::test]
async fn plain_output_carries_post_processed_content() {
    let result = extract_with_redaction(OutputFormat::Plain).await;

    assert_eq!(result.content, "Invoice for [REDACTED] is overdue.");
}

/// The other half of the guard, and the reason it is not simply "content changed -> drop
/// the rendering": a processor that maintains **both** surfaces has produced a rendering
/// that is current, so it must be kept rather than downgraded to plain text.
///
/// This is not hypothetical — the built-in redaction processor does exactly this
/// (`text/redaction/engine.rs` redacts `formatted_content` alongside `content`), and an
/// earlier version of this fix discarded its correctly-redacted markdown.
mod maintains_both_surfaces {
    use super::*;

    const MARKED: &str = "RENDERED-BY-PROCESSOR";
    const PROCESSOR_NAME: &str = "issue-331-dual-surface";

    struct DualSurfaceProcessor;

    impl Plugin for DualSurfaceProcessor {
        fn name(&self) -> &str {
            PROCESSOR_NAME
        }
    }

    #[async_trait]
    impl PostProcessor for DualSurfaceProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            result.content = result.content.replace(SECRET, REDACTED);
            if let Some(formatted) = result.formatted_content.as_mut() {
                *formatted = format!("{MARKED} {}", formatted.replace(SECRET, REDACTED));
            }
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Late
        }
    }

    #[tokio::test]
    async fn a_rendering_the_processor_kept_current_survives() {
        register_post_processor(std::sync::Arc::new(DualSurfaceProcessor))
            .expect("registering the processor should succeed");

        let config = ExtractionConfig {
            output_format: OutputFormat::Markdown,
            use_cache: false,
            ..Default::default()
        };
        let outcome = extract(
            ExtractInput::from_bytes(SOURCE_TEXT.as_bytes(), "text/plain", None),
            &config,
        )
        .await;

        unregister_post_processor(PROCESSOR_NAME).expect("unregistering the processor should succeed");

        let result = outcome
            .expect("extraction should succeed")
            .results
            .into_iter()
            .next()
            .expect("one input yields one result");

        assert!(
            result.content.contains(MARKED),
            "the processor's own rendering must survive, not be downgraded to plain text: {:?}",
            result.content
        );
        assert!(
            !result.content.contains(SECRET),
            "the surviving rendering must still be the post-processed one: {:?}",
            result.content
        );
        assert!(
            !result
                .processing_warnings
                .iter()
                .any(|warning| warning.source == "output_format"),
            "no downgrade warning is owed when the processor kept the rendering current: {:?}",
            result.processing_warnings
        );
    }
}
