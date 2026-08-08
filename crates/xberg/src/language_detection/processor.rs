//! Language detection post-processor.
//!
//! This module provides a PostProcessor plugin that detects languages in
//! extraction results and stores them in the result.

use crate::plugins::{Plugin, PostProcessor, ProcessingStage};
use crate::{ExtractedDocument, ExtractionConfig, Result, XbergError};
use async_trait::async_trait;

/// Post-processor that detects languages in document content.
///
/// This processor:
/// - Runs in the Early processing stage
/// - Only processes when `config.language_detection` is configured
/// - Stores detected languages in `result.detected_languages`
/// - Stores structured per-language confidence, proportion, script, and reliability in
///   `result.detected_language_confidences` (#261)
/// - Uses the whatlang library for detection
///
/// # Example
///
/// ```rust,no_run
/// use xberg::plugins::{Plugin, PostProcessor};
/// use xberg::language_detection::processor::LanguageDetector;
///
/// let processor = LanguageDetector;
/// assert_eq!(processor.name(), "language-detection");
/// ```
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy)]
pub struct LanguageDetector;

impl Plugin for LanguageDetector {
    fn name(&self) -> &str {
        "language-detection"
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
impl PostProcessor for LanguageDetector {
    async fn process(&self, result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
        let lang_config = match &config.language_detection {
            Some(cfg) => cfg,
            None => return Ok(()),
        };

        match super::detect_language_details(&result.content, lang_config)
            .map_err(|e| XbergError::Other(format!("Language detection failed: {}", e)))?
        {
            Some(details) => {
                result.detected_languages = Some(details.iter().map(|detail| detail.language.clone()).collect());
                result.detected_language_confidences = Some(details);
            }
            None => {
                result.detected_languages = None;
                result.detected_language_confidences = None;
            }
        }

        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Early
    }

    fn should_process(&self, _result: &ExtractedDocument, config: &ExtractionConfig) -> bool {
        config.language_detection.is_some()
    }

    fn estimated_duration_ms(&self, result: &ExtractedDocument) -> u64 {
        let text_length = result.content.len();
        (text_length / 1024).max(1) as u64
    }

    fn priority(&self) -> i32 {
        40
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::LanguageDetectionConfig;
    use std::borrow::Cow;

    #[tokio::test]
    async fn test_language_detector_processor() {
        let processor = LanguageDetector;
        let config = ExtractionConfig {
            language_detection: Some(LanguageDetectionConfig {
                enabled: true,
                min_confidence: 0.8,
                detect_multiple: false,
            }),
            ..Default::default()
        };

        let mut result = ExtractedDocument {
            content: "Hello world! This is a test of the language detection system.".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        processor.process(&mut result, &config).await.unwrap();

        assert!(result.detected_languages.is_some());
        let langs = result.detected_languages.unwrap();
        assert!(!langs.is_empty());
        assert_eq!(langs[0], "eng");
    }

    #[tokio::test]
    async fn test_language_detector_no_config() {
        let processor = LanguageDetector;
        let config = ExtractionConfig::default();

        let mut result = ExtractedDocument {
            content: "Hello world!".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        processor.process(&mut result, &config).await.unwrap();

        assert!(result.detected_languages.is_none());
    }

    #[test]
    fn test_language_detector_plugin_interface() {
        let processor = LanguageDetector;
        assert_eq!(processor.name(), "language-detection");
        assert!(!processor.version().is_empty());
        assert!(processor.initialize().is_ok());
        assert!(processor.shutdown().is_ok());
    }

    #[test]
    fn test_language_detector_stage() {
        let processor = LanguageDetector;
        assert_eq!(processor.processing_stage(), ProcessingStage::Early);
    }

    #[test]
    fn test_language_detector_should_process() {
        let processor = LanguageDetector;

        let result = ExtractedDocument {
            content: "Sample text".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        let config_with_lang = ExtractionConfig {
            language_detection: Some(LanguageDetectionConfig {
                enabled: true,
                min_confidence: 0.8,
                detect_multiple: false,
            }),
            ..Default::default()
        };
        assert!(processor.should_process(&result, &config_with_lang));

        let config_without_lang = ExtractionConfig::default();
        assert!(!processor.should_process(&result, &config_without_lang));
    }

    #[test]
    fn test_language_detector_estimated_duration() {
        let processor = LanguageDetector;

        let short_result = ExtractedDocument {
            content: "Short".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        let long_result = ExtractedDocument {
            content: "a".repeat(10000),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        let short_duration = processor.estimated_duration_ms(&short_result);
        let long_duration = processor.estimated_duration_ms(&long_result);

        assert!(long_duration > short_duration);
    }
}
