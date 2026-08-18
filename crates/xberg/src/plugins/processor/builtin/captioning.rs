//! Built-in Middle-stage post-processor that captions every
//! [`ExtractedImage`] using a VLM.
//!
//! Activates when [`ExtractionConfig::captioning`](crate::core::config::ExtractionConfig::captioning)
//! is `Some`. The processor walks `result.images`, and for each image whose
//! pixel area (`width * height`) is at least `min_image_area` it invokes
//! [`crate::llm::region_extractor::extract_region_with_vlm_usage`] in
//! [`RegionKind::Caption`]
//! mode. The caption is stored on [`ExtractedImage::caption`].
//!
//! Every VLM call's [`LlmUsage`](crate::types::LlmUsage) is appended to
//! [`ExtractedDocument::llm_usage`] so token / cost accounting carries over
//! into downstream telemetry.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::llm::region_extractor::{RegionKind, extract_region_with_vlm_usage};
use crate::plugins::{Plugin, PostProcessor, ProcessingStage, register_post_processor};
use crate::types::{ExtractedDocument, ExtractedImage};

/// Post-processor that captions every extracted image via a VLM.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptioningProcessor;

impl Plugin for CaptioningProcessor {
    fn name(&self) -> &str {
        "captioning"
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

#[async_trait]
impl PostProcessor for CaptioningProcessor {
    async fn process(&self, result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
        let Some(caption_config) = config.captioning.as_ref() else {
            return Ok(());
        };
        // Take the image vec out of `result` so the concurrent drain loop below can
        // record per-image warnings and usage on `result` without a borrow conflict.
        let Some(mut images) = result.images.take() else {
            result.processing_warnings.push(crate::types::ProcessingWarning {
                source: std::borrow::Cow::Borrowed("captioning"),
                message: std::borrow::Cow::Borrowed(
                    "captioning configured but no images were extracted; \
                     set config.images to enable image extraction from documents",
                ),
            });
            return Ok(());
        };
        if images.is_empty() {
            result.images = Some(images);
            return Ok(());
        }

        tracing::info!(
            target: "xberg::captioning",
            images = images.len(),
            model = %caption_config.llm.model,
            min_image_area = caption_config.min_image_area,
            "running per-image VLM captioning"
        );

        let min_area = u64::from(caption_config.min_image_area);
        let llm = Arc::new(caption_config.llm.clone());
        let prompt = caption_config.prompt.clone();

        use std::collections::VecDeque;
        use tokio::task::JoinSet;

        // Bound VLM captioning concurrency by the per-LLM request limit when set,
        // falling back to the configured thread budget for backward compatibility.
        // Mirrors the image-OCR path's replenished task set (#1378).
        let max_tasks =
            crate::core::config::concurrency::resolve_llm_concurrency(&caption_config.llm, config.concurrency.as_ref());

        type CaptionOutcome = crate::Result<(String, Option<crate::types::LlmUsage>)>;
        type PendingCaptionTask = (usize, bytes::Bytes, &'static str);
        let mut join_set: JoinSet<(usize, CaptionOutcome)> = JoinSet::new();
        let mut pending: VecDeque<PendingCaptionTask> = VecDeque::with_capacity(images.len());

        for (idx, image) in images.iter().enumerate() {
            if !image_is_caption_candidate(image, min_area) {
                continue;
            }
            pending.push_back((idx, image.data.clone(), mime_for_format(image.format.as_ref())));
        }

        let spawn_task = |join_set: &mut JoinSet<(usize, CaptionOutcome)>,
                          llm: Arc<crate::core::config::LlmConfig>,
                          prompt: Option<String>,
                          (idx, data, mime): PendingCaptionTask| {
            join_set.spawn(async move {
                let outcome =
                    extract_region_with_vlm_usage(data.as_ref(), mime, RegionKind::Caption, &llm, prompt.as_deref())
                        .await;
                (idx, outcome)
            });
        };

        while join_set.len() < max_tasks {
            let Some(task) = pending.pop_front() else {
                break;
            };
            spawn_task(&mut join_set, Arc::clone(&llm), prompt.clone(), task);
        }

        let mut captured_usage: Vec<crate::types::LlmUsage> = Vec::new();

        while let Some(join_result) = join_set.join_next().await {
            let (idx, outcome) = match join_result {
                Ok(value) => value,
                Err(join_error) => {
                    // A captioning task panicked. Record it and keep draining so the other
                    // images' captions — and the image vec itself, taken out above — are not
                    // lost by an early return before `result.images` is restored below.
                    result.processing_warnings.push(crate::types::ProcessingWarning {
                        source: std::borrow::Cow::Borrowed("captioning"),
                        message: std::borrow::Cow::Owned(format!("captioning task panicked: {join_error}")),
                    });
                    continue;
                }
            };

            match outcome {
                Ok((text, usage)) => {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        // Renderers emit `description` at the image placeholder but never
                        // read `caption`, so a caption produced here would otherwise never
                        // reach the output text. Mirror it into `description` when that is
                        // unset so the VLM caption is actually rendered at the image (#1340).
                        if images[idx].description.is_none() {
                            images[idx].description = Some(trimmed.clone());
                        }
                        images[idx].caption = Some(trimmed);
                    }
                    if let Some(mut usage) = usage {
                        if usage.source.is_empty() || usage.source == "vlm_ocr" {
                            usage.source = "captioning".to_string();
                        }
                        captured_usage.push(usage);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "xberg::captioning",
                        index = images[idx].image_index,
                        format = %images[idx].format,
                        error = %error,
                        "VLM caption call failed; image left without caption"
                    );
                    result.processing_warnings.push(crate::types::ProcessingWarning {
                        source: std::borrow::Cow::Borrowed("captioning"),
                        message: std::borrow::Cow::Owned(format!("Image {idx} captioning failed: {error}")),
                    });
                }
            }

            if let Some(task) = pending.pop_front() {
                spawn_task(&mut join_set, Arc::clone(&llm), prompt.clone(), task);
            }
        }

        result.images = Some(images);

        if !captured_usage.is_empty() {
            match result.llm_usage.as_mut() {
                Some(existing) => existing.extend(captured_usage),
                None => result.llm_usage = Some(captured_usage),
            }
        }
        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }

    fn should_process(&self, _result: &ExtractedDocument, config: &ExtractionConfig) -> bool {
        config.captioning.is_some()
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Pixel-area gate: skip masks and images smaller than the configured threshold.
///
/// Images with unknown dimensions are treated as candidates so format hints alone do
/// not silently suppress captioning when the extractor failed to populate width/height.
fn image_is_caption_candidate(image: &ExtractedImage, min_area: u64) -> bool {
    if image.is_mask {
        return false;
    }
    match (image.width, image.height) {
        (Some(w), Some(h)) => u64::from(w) * u64::from(h) >= min_area,
        _ => true,
    }
}

/// Map an [`ExtractedImage::format`] string into a MIME type the VLM call accepts.
fn mime_for_format(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => "image/png",
    }
}

/// Register the default captioning post-processor with the global registry.
#[cfg_attr(alef, alef(skip))]
pub fn register() -> Result<()> {
    register_post_processor(Arc::new(CaptioningProcessor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{CaptioningConfig, LlmConfig};
    use bytes::Bytes;
    use std::borrow::Cow;

    fn caption_config(min_image_area: u32) -> CaptioningConfig {
        CaptioningConfig {
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
            prompt: None,
            min_image_area,
        }
    }

    fn image_with(width: Option<u32>, height: Option<u32>, is_mask: bool) -> ExtractedImage {
        ExtractedImage {
            data: Bytes::from_static(&[]),
            format: Cow::Borrowed("png"),
            width,
            height,
            is_mask,
            ..Default::default()
        }
    }

    #[test]
    fn processor_metadata_is_correct() {
        let p = CaptioningProcessor;
        assert_eq!(p.name(), "captioning");
        assert_eq!(p.processing_stage(), ProcessingStage::Middle);
    }

    #[test]
    fn should_process_only_when_config_present() {
        let p = CaptioningProcessor;
        let result = ExtractedDocument {
            content: "x".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };
        assert!(!p.should_process(&result, &ExtractionConfig::default()));

        let cfg = ExtractionConfig {
            captioning: Some(caption_config(1000)),
            ..Default::default()
        };
        assert!(p.should_process(&result, &cfg));
    }

    #[test]
    fn masks_are_never_candidates() {
        let image = image_with(Some(2000), Some(2000), true);
        assert!(!image_is_caption_candidate(&image, 1_000));
    }

    #[test]
    fn small_images_are_skipped() {
        let image = image_with(Some(10), Some(10), false);
        assert!(!image_is_caption_candidate(&image, 1_000));
    }

    #[test]
    fn large_images_pass_threshold() {
        let image = image_with(Some(64), Some(64), false);
        assert!(image_is_caption_candidate(&image, 1_000));
    }

    #[test]
    fn unknown_dimensions_pass_through() {
        let image = image_with(None, None, false);
        assert!(image_is_caption_candidate(&image, 1_000));
    }

    #[tokio::test]
    async fn warns_when_no_images_extracted() {
        let p = CaptioningProcessor;
        let cfg = ExtractionConfig {
            captioning: Some(caption_config(1000)),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: "x".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            ..Default::default()
        };
        p.process(&mut result, &cfg).await.unwrap();
        let warnings = result.processing_warnings;
        assert!(
            warnings.iter().any(|w| w.source == "captioning"),
            "expected captioning warning when images is None; got: {warnings:?}"
        );
    }

    #[test]
    fn mime_mapping_covers_common_formats() {
        assert_eq!(mime_for_format("png"), "image/png");
        assert_eq!(mime_for_format("JPG"), "image/jpeg");
        assert_eq!(mime_for_format("jpeg"), "image/jpeg");
        assert_eq!(mime_for_format("webp"), "image/webp");
        assert_eq!(mime_for_format("tiff"), "image/tiff");
        assert_eq!(mime_for_format("bmp"), "image/bmp");
        assert_eq!(mime_for_format("gif"), "image/gif");
        assert_eq!(mime_for_format("unknown"), "image/png");
    }
}
