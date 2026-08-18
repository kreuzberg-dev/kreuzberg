//! Centralized image OCR processing.
//!
//! Provides a shared function for processing extracted images with OCR,
//! used by DOCX, PPTX, Jupyter, Markdown, and other extractors.
//!
//! # Recursion Prevention
//!
//! The OCR results produced here set `images: None` to prevent any
//! downstream consumer from triggering further image extraction on
//! OCR output. This breaks the potential cycle:
//! document → extract images → OCR images → (no further image extraction).
//!
//! # Concurrency
//!
//! Image OCR tasks within one extraction operation are processed with a bounded
//! concurrency limit derived from the VLM request limit, when configured, or the
//! general thread budget otherwise to prevent resource exhaustion when documents
//! contain many embedded images.

use crate::types::{ExtractedDocument, ExtractedImage};

/// Process extracted images with OCR if configured.
///
/// For each image, spawns an async OCR task using the backend from the registry
/// and stores the result in `image.ocr_result`. If OCR is not configured or
/// fails for an individual image, that image's `ocr_result` remains `None`.
///
/// This function is the single shared implementation used by all
/// document extractors (DOCX, PPTX, Jupyter, Markdown, etc.).
///
/// # Recursion Safety
///
/// The produced `ExtractedDocument` for each image explicitly sets
/// `images: None`, preventing further image extraction cycles when
/// OCR results are consumed by archive or recursive extraction paths.
///
/// # Concurrency
///
/// Concurrency within the current extraction is bounded by the configured VLM
/// request limit (falling back to the thread budget) using a replenished task set,
/// so queued images do not create an unbounded number of futures. Concurrent
/// document extractions each enforce their own limit.
#[cfg(all(feature = "ocr", feature = "tokio-runtime"))]
pub(crate) async fn process_images_with_ocr(
    mut images: Vec<ExtractedImage>,
    config: &crate::core::config::ExtractionConfig,
    warnings: &mut Vec<crate::types::ProcessingWarning>,
) -> crate::Result<Vec<ExtractedImage>> {
    if images.is_empty() || config.ocr.is_none() {
        return Ok(images);
    }

    let ocr_config = config.ocr.as_ref().unwrap();
    let output_format = config.output_format.clone();
    let acceleration = ocr_config.acceleration.clone();

    use std::collections::VecDeque;
    use tokio::task::JoinSet;

    let max_tasks = crate::core::config::concurrency::resolve_ocr_concurrency(ocr_config, config.concurrency.as_ref());

    type OcrTaskResult = (usize, crate::Result<ExtractedDocument>);
    type PendingOcrTask = (usize, bytes::Bytes, crate::core::config::OcrConfig);
    let mut join_set: JoinSet<OcrTaskResult> = JoinSet::new();
    let mut pending: VecDeque<PendingOcrTask> = VecDeque::with_capacity(images.len());

    for (idx, image) in images.iter().enumerate() {
        let image_data = image.data.clone();
        let mut ocr_config_clone = ocr_config.clone();
        ocr_config_clone.output_format = Some(output_format.clone());
        ocr_config_clone.acceleration = acceleration.clone();
        pending.push_back((idx, image_data, ocr_config_clone));
    }

    let spawn_task = |join_set: &mut JoinSet<OcrTaskResult>, (idx, image_data, ocr_config_clone): PendingOcrTask| {
        join_set.spawn(async move {
            let backend = {
                let registry = crate::plugins::registry::get_ocr_backend_registry();
                let registry = registry.read();
                match registry.get(&ocr_config_clone.backend) {
                    Ok(b) => b.clone(),
                    Err(e) => {
                        return (
                            idx,
                            Err(crate::XbergError::Ocr {
                                message: format!("OCR backend '{}' not found: {}", ocr_config_clone.backend, e),
                                source: None,
                            }),
                        );
                    }
                }
            };

            let ocr_result = backend.process_image(&image_data, &ocr_config_clone).await;
            (idx, ocr_result)
        });
    };

    while join_set.len() < max_tasks {
        let Some(task) = pending.pop_front() else {
            break;
        };
        spawn_task(&mut join_set, task);
    }

    while let Some(join_result) = join_set.join_next().await {
        let (idx, ocr_result) = join_result.map_err(|e| crate::XbergError::Ocr {
            message: format!("OCR task panicked: {}", e),
            source: None,
        })?;

        match ocr_result {
            Ok(extraction_result) => {
                // Keep the backend's result whole. Rebuilding it field-by-field silently
                // dropped everything the backend populated besides content/mime_type/
                // ocr_elements — tables, metadata (OCR language, PSM, confidence),
                // formulas, llm_usage (VLM cost accounting), detected_languages and
                // processing_warnings. The PDF inline-image path already stores the
                // backend result unmodified; mirror it here.
                let mut ocr_document = extraction_result;
                // Recursion guard: OCR output must never carry nested images, or an
                // archive/recursive consumer would extract images out of OCR output.
                ocr_document.images = None;
                images[idx].ocr_result = Some(Box::new(ocr_document));
            }
            Err(e) => {
                warnings.push(crate::types::ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("image_ocr"),
                    message: std::borrow::Cow::Owned(format!("Image {} OCR failed: {}", idx, e)),
                });
                images[idx].ocr_result = None;
            }
        }

        if let Some(task) = pending.pop_front() {
            spawn_task(&mut join_set, task);
        }
    }

    Ok(images)
}

#[cfg(all(test, feature = "ocr", feature = "tokio-runtime"))]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use bytes::Bytes;
    use tokio::sync::Barrier;

    use super::*;
    use crate::core::config::{ConcurrencyConfig, LlmConfig, OcrConfig, VlmFallbackPolicy};
    use crate::plugins::{OcrBackend, OcrBackendType, Plugin};

    const BACKEND_NAME: &str = "vlm-concurrency-test-backend";

    struct RegistrationGuard;

    impl Drop for RegistrationGuard {
        fn drop(&mut self) {
            let _ = crate::plugins::unregister_ocr_backend(BACKEND_NAME);
        }
    }

    struct MeasuringBackend {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        first_wave: Arc<Barrier>,
    }

    impl Plugin for MeasuringBackend {
        fn name(&self) -> &str {
            BACKEND_NAME
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> crate::Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> crate::Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OcrBackend for MeasuringBackend {
        async fn process_image(&self, _image_bytes: &[u8], _config: &OcrConfig) -> crate::Result<ExtractedDocument> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);

            // The first two calls meet here. A regression that permits only one task
            // times out; one that launches more than two is captured by `peak`.
            if call_index < 2 {
                self.first_wave.wait().await;
            }

            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedDocument {
                content: format!("image {call_index}"),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }

        fn supports_language(&self, _lang: &str) -> bool {
            true
        }

        fn backend_type(&self) -> OcrBackendType {
            OcrBackendType::Custom
        }
    }

    #[tokio::test]
    async fn per_llm_limit_bounds_actual_in_flight_image_ocr_requests() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        crate::plugins::register_ocr_backend(Arc::new(MeasuringBackend {
            active: Arc::clone(&active),
            peak: Arc::clone(&peak),
            calls: Arc::clone(&calls),
            first_wave: Arc::new(Barrier::new(2)),
        }))
        .expect("register measuring OCR backend");
        let _registration = RegistrationGuard;

        let config = crate::core::config::ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                vlm_fallback: VlmFallbackPolicy::Always,
                vlm_config: Some(LlmConfig {
                    model: "test/model".to_string(),
                    max_concurrency: Some(2),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            concurrency: Some(ConcurrencyConfig { max_threads: Some(8) }),
            ..Default::default()
        };
        let images = (0..6)
            .map(|_| ExtractedImage {
                data: Bytes::from_static(b"image"),
                ..Default::default()
            })
            .collect();
        let mut warnings = Vec::new();

        let processed = tokio::time::timeout(
            Duration::from_secs(5),
            process_images_with_ocr(images, &config, &mut warnings),
        )
        .await
        .expect("configured concurrency should allow the first wave to run")
        .expect("image OCR should succeed");

        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert!(warnings.is_empty());
        assert!(processed.iter().all(|image| image.ocr_result.is_some()));
    }
}
