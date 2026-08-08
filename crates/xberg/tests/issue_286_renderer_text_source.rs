//! Regression tests for #286: renderers must never be handed pre-post-processing text.
//!
//! `run_pipeline` stores a clone of the extractor's `InternalDocument` on
//! `ExtractedDocument::internal_document` **before** the post-processor stages run, and no
//! stage ever writes back into that tree. Two consumers read it: the blanket
//! `impl<T: InternalRenderer> Renderer for T` behind the public `Renderer::render_result`,
//! and `transform_extraction_result_to_elements`, which prefers the tree over
//! `content`/`pages` when building the public `elements` — the same `elements` the renderer
//! registry attaches to the document it hands foreign renderers.
//!
//! So once a post-processor rewrote `content`, a renderer's output for a document
//! disagreed with that same document's `content`: it emitted the original, un-rewritten
//! text. These tests pin the invariant that it no longer can.

use std::sync::Arc;

use async_trait::async_trait;
use xberg::plugins::Plugin;
use xberg::{
    ExtractInput, ExtractedDocument, ExtractionConfig, PostProcessor, ProcessingStage, Renderer, Result, ResultFormat,
    XbergError, extract, register_post_processor, unregister_post_processor,
};

/// The token the post-processor removes. Its presence in rendered output is proof that the
/// renderer read text that never went through post-processing.
const SECRET: &str = "ACCOUNT-1234";
const REDACTED: &str = "[REDACTED]";
const SOURCE_TEXT: &str = "Invoice for ACCOUNT-1234 is overdue.";
const POST_PROCESSED_TEXT: &str = "Invoice for [REDACTED] is overdue.";
const UNTOUCHED_TEXT: &str = "Quarterly report for the Berlin office.";
const PROCESSOR_NAME: &str = "issue-286-redactor";

/// Late-stage post-processor that rewrites `content` (and the per-page content) the way the
/// built-in redaction processor does. It is a no-op on documents that do not carry
/// [`SECRET`], so it cannot perturb the other test in this binary.
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

/// A public renderer of the shape a foreign (binding-registered) renderer has: it can only
/// see the public surface of `ExtractedDocument`, and it renders from `elements` — the
/// field the renderer registry populates from `internal_document`.
///
/// It errors rather than falling back to `content` so the assertions below cannot pass by
/// silently degrading to "just echo the content".
struct ElementTextRenderer;

impl Plugin for ElementTextRenderer {
    fn name(&self) -> &str {
        "issue-286-element-text"
    }
}

impl Renderer for ElementTextRenderer {
    fn render_result(&self, result: &ExtractedDocument) -> Result<String> {
        let elements = result
            .elements
            .as_ref()
            .ok_or_else(|| XbergError::Other("renderer received a document with no elements".to_string()))?;
        Ok(elements.iter().map(|e| e.text.as_str()).collect::<Vec<_>>().join("\n"))
    }
}

fn element_based_config() -> ExtractionConfig {
    ExtractionConfig {
        // The cache round-trips through serde, which drops the `#[serde(skip)]`
        // `internal_document`; a cache hit would make these tests pass without the fix.
        use_cache: false,
        result_format: ResultFormat::ElementBased,
        ..Default::default()
    }
}

async fn extract_plain_text(text: &str, filename: &str, config: &ExtractionConfig) -> ExtractedDocument {
    let outcome = extract(
        ExtractInput::from_bytes(text.as_bytes().to_vec(), "text/plain", Some(filename.to_string())),
        config,
    )
    .await
    .expect("extraction succeeds");

    outcome.results.into_iter().next().expect("one input yields one result")
}

#[tokio::test]
async fn renderer_output_reflects_post_processed_text_not_the_preserved_element_tree() {
    register_post_processor(Arc::new(RedactingProcessor)).expect("post-processor registers");

    let document = extract_plain_text(SOURCE_TEXT, "issue_286_redacted.txt", &element_based_config()).await;

    unregister_post_processor(PROCESSOR_NAME).expect("post-processor unregisters");

    assert_eq!(
        document.content.trim(),
        POST_PROCESSED_TEXT,
        "the Late post-processor must have rewritten content before the renderer sees the document"
    );

    let rendered = Renderer::render_result(&ElementTextRenderer, &document).expect("renderer produces output");

    assert_eq!(rendered, POST_PROCESSED_TEXT);
    assert!(
        !rendered.contains(SECRET),
        "renderer emitted pre-post-processing text: {rendered}"
    );
    assert_eq!(
        rendered,
        document.content.trim(),
        "a renderer's output must agree with the same document's content"
    );
}

/// The fix must not amount to "always throw the element tree away": a document whose
/// content post-processing left alone still gets a populated, content-consistent
/// `elements` list.
#[tokio::test]
async fn elements_remain_populated_when_post_processing_leaves_content_unchanged() {
    let document = extract_plain_text(UNTOUCHED_TEXT, "issue_286_untouched.txt", &element_based_config()).await;

    let rendered = Renderer::render_result(&ElementTextRenderer, &document).expect("renderer produces output");

    assert_eq!(rendered, UNTOUCHED_TEXT);
    assert_eq!(rendered, document.content.trim());
}
