//! Regression test for #58: the captioning prepass merged only a fragment of what
//! the captioning post-processor produced back into the document.
//!
//! Two defects are covered:
//!
//! 1. The image merge used `doc.images.iter_mut().zip(captioned_images)`, which
//!    silently truncates to the shorter side. A captioner that adds (or removes) an
//!    image had that image dropped and the pairing corrupted.
//! 2. Everything the processor produced besides `description` / `caption` /
//!    `processing_warnings` / `llm_usage` was discarded — `metadata`, `uris`,
//!    `content`, and `entities`.
//!
//! The test drives the real `run_pipeline` with a stub post-processor registered
//! under the `captioning` name, so no VLM, network, or fixture is involved.

use async_trait::async_trait;
use serial_test::serial;
use std::borrow::Cow;
use std::sync::Arc;
use xberg::Result;
use xberg::core::config::{CaptioningConfig, ExtractionConfig, LlmConfig};
use xberg::core::pipeline::{clear_processor_cache, run_pipeline};
use xberg::internal::{ElementKind, InternalDocument, InternalElement};
use xberg::plugins::registry::get_post_processor_registry;
use xberg::plugins::{Plugin, PostProcessor, ProcessingStage};
use xberg::types::{Entity, EntityCategory, ExtractedDocument, ExtractedImage, ExtractedUri, UriKind};

const ADDED_IMAGE_DESCRIPTION: &str = "added-by-captioner";
const CAPTIONED_CONTENT: &str = "content authored by the captioning prepass";
const CAPTION_URL: &str = "https://example.com/figure";
const ENTITY_TEXT: &str = "Ada";

/// Stub captioner: renames every image, appends one extra image, and populates the
/// document-level fields a real captioning-stage processor may produce.
struct StubCaptioner;

impl Plugin for StubCaptioner {
    fn name(&self) -> &str {
        "captioning"
    }

    fn version(&self) -> String {
        "1.0.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl PostProcessor for StubCaptioner {
    async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
        let mut images = result.images.take().unwrap_or_default();
        for (idx, image) in images.iter_mut().enumerate() {
            image.description = Some(format!("caption-{idx}"));
            image.caption = Some(format!("caption-{idx}"));
        }
        images.push(ExtractedImage {
            data: bytes::Bytes::from_static(b"added-image-bytes"),
            format: Cow::Borrowed("png"),
            image_index: images.len() as u32,
            description: Some(ADDED_IMAGE_DESCRIPTION.to_string()),
            caption: Some(ADDED_IMAGE_DESCRIPTION.to_string()),
            ..Default::default()
        });
        result.images = Some(images);

        result
            .metadata
            .additional
            .insert(Cow::Borrowed("captioning_stub"), serde_json::json!("ran"));
        result.uris = Some(vec![ExtractedUri {
            url: CAPTION_URL.to_string(),
            label: None,
            page: None,
            kind: UriKind::Image,
        }]);
        result.entities = Some(vec![Entity {
            category: EntityCategory::Person,
            text: ENTITY_TEXT.to_string(),
            start: 0,
            end: 3,
            confidence: None,
        }]);
        result.content = CAPTIONED_CONTENT.to_string();
        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }

    fn should_process(&self, _result: &ExtractedDocument, config: &ExtractionConfig) -> bool {
        config.captioning.is_some()
    }
}

fn install_stub_captioner() {
    let registry = get_post_processor_registry();
    {
        let mut reg = registry.write();
        let _ = reg.shutdown_all();
        reg.register(Arc::new(StubCaptioner)).expect("stub captioner registers");
    }
    let _ = clear_processor_cache();
}

fn doc_with_images(count: u32) -> InternalDocument {
    let mut doc = InternalDocument::new("markdown");
    doc.mime_type = "text/markdown".to_string();
    doc.elements
        .push(InternalElement::text(ElementKind::Paragraph, "body text", 0));
    for image_index in 0..count {
        doc.images.push(ExtractedImage {
            data: bytes::Bytes::from_static(b"source-image-bytes"),
            format: Cow::Borrowed("png"),
            image_index,
            ..Default::default()
        });
    }
    doc
}

fn captioning_config() -> ExtractionConfig {
    ExtractionConfig {
        captioning: Some(CaptioningConfig {
            llm: LlmConfig {
                model: "stub/model".to_string(),
                ..Default::default()
            },
            prompt: None,
            min_image_area: 0,
        }),
        use_cache: false,
        ..Default::default()
    }
}

/// An image appended by the captioning processor must reach the final result, and
/// the surviving images must keep their own captions (no `zip` truncation).
#[tokio::test]
#[serial]
async fn captioning_prepass_keeps_images_added_by_the_processor() {
    install_stub_captioner();

    let result = run_pipeline(doc_with_images(2), &captioning_config())
        .await
        .expect("pipeline succeeds");

    let images = result.images.expect("images must be present");
    assert_eq!(
        images.len(),
        3,
        "captioner returned 3 images (2 captioned + 1 added); prepass must not truncate"
    );
    assert_eq!(images[0].description.as_deref(), Some("caption-0"));
    assert_eq!(images[0].caption.as_deref(), Some("caption-0"));
    assert_eq!(images[1].description.as_deref(), Some("caption-1"));
    assert_eq!(images[1].caption.as_deref(), Some("caption-1"));
    assert_eq!(images[2].description.as_deref(), Some(ADDED_IMAGE_DESCRIPTION));
    assert_eq!(images[2].data.as_ref(), b"added-image-bytes");
}

/// `metadata`, `uris`, `content`, and `entities` produced by the prepass processor
/// must survive into the final result.
#[tokio::test]
#[serial]
async fn captioning_prepass_keeps_document_level_fields() {
    install_stub_captioner();

    let result = run_pipeline(doc_with_images(1), &captioning_config())
        .await
        .expect("pipeline succeeds");

    assert_eq!(
        result
            .metadata
            .additional
            .get("captioning_stub")
            .and_then(serde_json::Value::as_str),
        Some("ran"),
        "metadata written by the captioning prepass must survive"
    );

    let uris = result.uris.expect("uris must be present");
    assert_eq!(uris.len(), 1);
    assert_eq!(uris[0].url, CAPTION_URL);
    assert_eq!(uris[0].kind, UriKind::Image);

    let entities = result.entities.expect("entities must be present");
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].text, ENTITY_TEXT);
    assert_eq!(entities[0].category, EntityCategory::Person);

    assert_eq!(result.content, CAPTIONED_CONTENT);
}
