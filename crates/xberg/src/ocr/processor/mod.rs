//! OCR processor implementation using Tesseract.
//!
//! This module has been split into focused submodules for better organization:
//! - `validation` - Image and configuration validation
//! - `config` - Configuration hashing and Tesseract variables
//! - `execution` - Core OCR execution logic

mod api_pool;
mod config;
mod execution;
pub(crate) mod validation;

pub(crate) use api_pool::MAX_TESSERACT_APIS;

use crate::ocr::cache::OcrCache;
use crate::ocr::error::OcrError;
#[cfg(test)]
use crate::ocr::types::BatchItemResult;
use crate::ocr::types::TesseractConfig;
use crate::types::OcrExtractionResult;
#[cfg(feature = "otel")]
use std::time::Instant;

/// Tesseract-backed OCR processor with persistent result caching.
#[cfg_attr(alef, alef(skip))]
pub struct OcrProcessor {
    cache: OcrCache,
    api_pool: std::sync::Arc<api_pool::TesseractApiPool>,
}

impl OcrProcessor {
    pub(crate) fn new(cache_dir: Option<std::path::PathBuf>) -> Result<Self, OcrError> {
        let cache = OcrCache::new(cache_dir)?;
        Ok(Self {
            cache,
            api_pool: api_pool::TesseractApiPool::new(),
        })
    }

    pub(crate) fn process_image(
        &self,
        image_bytes: &[u8],
        config: &TesseractConfig,
    ) -> Result<OcrExtractionResult, OcrError> {
        #[cfg(feature = "otel")]
        let span = crate::telemetry::spans::ocr_span("tesseract", &config.language);
        #[cfg(feature = "otel")]
        let _guard = span.enter();
        #[cfg(feature = "otel")]
        let start = Instant::now();

        let result = execution::process_image_with_cache(image_bytes, config, &self.cache, &self.api_pool, None);

        #[cfg(feature = "otel")]
        {
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
            crate::telemetry::metrics::get_metrics().ocr_duration_ms.record(
                duration_ms,
                &[
                    opentelemetry::KeyValue::new(crate::telemetry::conventions::OCR_BACKEND, "tesseract"),
                    opentelemetry::KeyValue::new(crate::telemetry::conventions::OCR_LANGUAGE, config.language.clone()),
                ],
            );
        }

        result
    }

    /// Process an image with OCR and respect the output format from ExtractionConfig.
    ///
    /// This variant allows specifying an output format (Plain, Markdown, Djot) which
    /// affects how the OCR result's mime_type is set when markdown output is requested.
    pub(crate) fn process_image_with_format(
        &self,
        image_bytes: &[u8],
        config: &TesseractConfig,
        output_format: crate::core::config::OutputFormat,
    ) -> Result<OcrExtractionResult, OcrError> {
        #[cfg(feature = "otel")]
        let span = crate::telemetry::spans::ocr_span("tesseract", &config.language);
        #[cfg(feature = "otel")]
        let _guard = span.enter();
        #[cfg(feature = "otel")]
        let start = Instant::now();

        let result =
            execution::process_image_with_cache(image_bytes, config, &self.cache, &self.api_pool, Some(output_format));

        #[cfg(feature = "otel")]
        {
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
            crate::telemetry::metrics::get_metrics().ocr_duration_ms.record(
                duration_ms,
                &[
                    opentelemetry::KeyValue::new(crate::telemetry::conventions::OCR_BACKEND, "tesseract"),
                    opentelemetry::KeyValue::new(crate::telemetry::conventions::OCR_LANGUAGE, config.language.clone()),
                ],
            );
        }

        result
    }

    pub(crate) fn clear_cache(&self) -> Result<(), OcrError> {
        self.cache.clear()
    }

    #[cfg(test)]
    pub(crate) fn get_cache_stats(&self) -> Result<super::cache::OcrCacheStats, OcrError> {
        self.cache.get_stats()
    }

    pub(crate) fn process_image_file(
        &self,
        file_path: &str,
        config: &TesseractConfig,
    ) -> Result<OcrExtractionResult, OcrError> {
        execution::process_image_file_with_cache(file_path, config, &self.cache, &self.api_pool, None)
    }

    /// Process a file with OCR and respect the output format from ExtractionConfig.
    ///
    /// This variant allows specifying an output format (Plain, Markdown, Djot) which
    /// affects how the OCR result's mime_type is set when markdown output is requested.
    pub(crate) fn process_image_file_with_format(
        &self,
        file_path: &str,
        config: &TesseractConfig,
        output_format: crate::core::config::OutputFormat,
    ) -> Result<OcrExtractionResult, OcrError> {
        execution::process_image_file_with_cache(file_path, config, &self.cache, &self.api_pool, Some(output_format))
    }

    /// Process multiple image files in parallel using Rayon.
    ///
    /// This method processes OCR operations in parallel across CPU cores for improved throughput.
    /// Results are returned in the same order as the input file paths.
    #[cfg(test)]
    pub(crate) fn process_image_files_batch(
        &self,
        file_paths: Vec<String>,
        config: &TesseractConfig,
    ) -> Vec<BatchItemResult> {
        execution::process_image_files_batch(file_paths, config, &self.cache, &self.api_pool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_test_config() -> TesseractConfig {
        TesseractConfig {
            output_format: "text".to_string(),
            enable_table_detection: false,
            use_cache: false,
            ..TesseractConfig::default()
        }
    }

    #[test]
    fn test_processor_creation() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf()));
        assert!(processor.is_ok());
    }

    #[test]
    fn test_processor_creation_default_cache_dir() {
        let processor = OcrProcessor::new(None);
        assert!(processor.is_ok());
    }

    #[test]
    fn test_cache_operations() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();

        assert!(processor.clear_cache().is_ok());

        let stats = processor.get_cache_stats();
        assert!(stats.is_ok());
    }

    #[test]
    fn test_process_image_file_nonexistent() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let result = processor.process_image_file("/nonexistent/file.png", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to read file"));
    }

    #[test]
    fn test_process_image_files_batch_empty() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let results = processor.process_image_files_batch(vec![], &config);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_process_image_invalid_image_data() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let invalid_data = vec![0, 1, 2, 3, 4];
        let result = processor.process_image(&invalid_data, &config);

        assert!(result.is_err());
    }

    #[test]
    fn test_process_image_files_batch_single_file() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let results = processor.process_image_files_batch(vec!["/nonexistent.png".to_string()], &config);
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.is_some());
        assert!(results[0].result.is_none());
    }

    #[test]
    fn test_process_image_files_batch_multiple_files() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let file_paths = vec![
            "/nonexistent1.png".to_string(),
            "/nonexistent2.png".to_string(),
            "/nonexistent3.png".to_string(),
        ];

        let results = processor.process_image_files_batch(file_paths, &config);
        assert_eq!(results.len(), 3);

        for result in &results {
            assert!(!result.success);
            assert!(result.error.is_some());
            assert!(result.result.is_none());
        }
    }

    #[test]
    fn test_batch_item_result_structure() {
        use std::collections::HashMap;

        let success_result = BatchItemResult {
            file_path: "test.png".to_string(),
            success: true,
            result: Some(OcrExtractionResult {
                content: "test".to_string(),
                mime_type: "text/plain".to_string(),
                metadata: HashMap::new(),
                tables: vec![],
                ocr_elements: None,
                internal_document: None,
            }),
            error: None,
        };

        assert!(success_result.success);
        assert!(success_result.result.is_some());
        assert!(success_result.error.is_none());

        let error_result = BatchItemResult {
            file_path: "error.png".to_string(),
            success: false,
            result: None,
            error: Some("Test error".to_string()),
        };

        assert!(!error_result.success);
        assert!(error_result.result.is_none());
        assert!(error_result.error.is_some());
    }

    #[test]
    fn test_process_image_files_batch_preserves_order() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let config = create_test_config();

        let file_paths = vec![
            "file1.png".to_string(),
            "file2.png".to_string(),
            "file3.png".to_string(),
        ];

        let results = processor.process_image_files_batch(file_paths.clone(), &config);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].file_path, "file1.png");
        assert_eq!(results[1].file_path, "file2.png");
        assert_eq!(results[2].file_path, "file3.png");
    }

    #[test]
    fn test_process_image_with_cache_disabled() {
        let temp_dir = tempdir().unwrap();
        let processor = OcrProcessor::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let mut config = create_test_config();
        config.use_cache = false;

        let invalid_data = vec![0, 1, 2, 3];
        let result = processor.process_image(&invalid_data, &config);

        assert!(result.is_err());
    }
}
