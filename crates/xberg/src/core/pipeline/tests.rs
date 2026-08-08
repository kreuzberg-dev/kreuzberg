//! Pipeline orchestration tests.

use super::*;
use crate::core::config::OutputFormat;
use crate::types::Metadata;
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use serial_test::serial;
use std::borrow::Cow;

/// Build an `InternalDocument` with a single paragraph element for pipeline tests.
fn make_doc(content: &str, mime: &str) -> InternalDocument {
    let mut doc = InternalDocument::new("plain");
    doc.mime_type = mime.to_string();
    if !content.is_empty() {
        doc.push_element(InternalElement::text(ElementKind::Paragraph, content, 0));
    }
    doc
}

/// Build an `InternalDocument` with content, mime, and custom metadata.
fn make_doc_with_metadata(content: &str, mime: &str, metadata: Metadata) -> InternalDocument {
    let mut doc = make_doc(content, mime);
    doc.metadata = metadata;
    doc
}

const VALIDATION_MARKER_KEY: &str = "registry_validation_marker";
#[cfg(feature = "quality")]
const QUALITY_VALIDATION_MARKER: &str = "quality_validation_test";
const POSTPROCESSOR_VALIDATION_MARKER: &str = "postprocessor_validation_test";
const ORDER_VALIDATION_MARKER: &str = "order_validation_test";

/// Ensure the quality processor is registered and cache is fresh.
/// Needed because other tests may call `shutdown_all()` on the registry,
/// and the `OnceLock` in `initialize_features` prevents re-registration.
#[cfg(feature = "quality")]
fn ensure_quality_processor() {
    let registry = crate::plugins::registry::get_post_processor_registry();
    let mut reg = registry.write();
    let _ = reg.register(std::sync::Arc::new(crate::text::QualityProcessor));
    drop(reg);
    let _ = clear_processor_cache();
}

#[tokio::test]
#[serial]
async fn test_run_pipeline_basic() {
    let mut doc = make_doc("test", "text/plain");
    doc.metadata.additional.insert(
        Cow::Borrowed(VALIDATION_MARKER_KEY),
        serde_json::json!(ORDER_VALIDATION_MARKER),
    );
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.content, "test");
}

#[tokio::test]
#[serial]
#[cfg(feature = "quality")]
async fn test_pipeline_with_quality_processing() {
    ensure_quality_processor();
    let doc = make_doc("This is a test document with some meaningful content.", "text/plain");
    let config = ExtractionConfig {
        enable_quality_processing: true,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(processed.quality_score.is_some());
}

#[tokio::test]
#[serial]
async fn test_pipeline_without_quality_processing() {
    let doc = make_doc("test", "text/plain");
    let config = ExtractionConfig {
        enable_quality_processing: false,
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(processed.quality_score.is_none());
}

#[tokio::test]
#[serial]
#[cfg(feature = "chunking")]
async fn test_pipeline_with_chunking() {
    let doc = make_doc(
        &"This is a long text that should be chunked. ".repeat(100),
        "text/plain",
    );
    let config = ExtractionConfig {
        chunking: Some(crate::ChunkingConfig {
            max_characters: 500,
            overlap: 50,
            trim: true,
            chunker_type: crate::ChunkerType::Text,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    let chunks = processed.chunks.as_ref().expect("chunks should be present");
    assert!(chunks.len() > 1);
}

#[tokio::test]
#[serial]
async fn test_pipeline_without_chunking() {
    let doc = make_doc("test", "text/plain");
    let config = ExtractionConfig {
        chunking: None,
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(processed.chunks.is_none());
}

#[tokio::test]
#[serial]
async fn test_pipeline_preserves_metadata() {
    use ahash::AHashMap;
    let mut additional = AHashMap::new();
    additional.insert(Cow::Borrowed("source"), serde_json::json!("test"));
    additional.insert(Cow::Borrowed("page"), serde_json::json!(1));

    let doc = make_doc_with_metadata(
        "test",
        "text/plain",
        Metadata {
            additional,
            ..Default::default()
        },
    );
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(
        processed.metadata.additional.get("source").unwrap(),
        &serde_json::json!("test")
    );
    assert_eq!(
        processed.metadata.additional.get("page").unwrap(),
        &serde_json::json!(1)
    );
}

#[tokio::test]
#[serial]
async fn test_pipeline_preserves_tables() {
    use crate::types::Table;

    let table = Table {
        cells: vec![vec!["A".to_string(), "B".to_string()]],
        markdown: "| A | B |".to_string(),
        page_number: 0,
        bounding_box: None,
        ..Default::default()
    };

    let mut doc = make_doc("test", "text/plain");
    doc.tables.push(table);
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.tables.len(), 1);
    assert_eq!(processed.tables[0].cells.len(), 1);
}

#[tokio::test]
#[serial]
async fn test_pipeline_empty_content() {
    {
        let registry = crate::plugins::registry::get_post_processor_registry();
        registry.write().shutdown_all().unwrap();
    }
    {
        let registry = crate::plugins::registry::get_validator_registry();
        registry.write().shutdown_all().unwrap();
    }

    let doc = make_doc("", "text/plain");
    let config = ExtractionConfig::default();

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.content, "");
}

#[tokio::test]
#[serial]
#[cfg(feature = "chunking")]
async fn test_pipeline_with_all_features() {
    #[cfg(feature = "quality")]
    ensure_quality_processor();
    let doc = make_doc(&"This is a comprehensive test document. ".repeat(50), "text/plain");
    let config = ExtractionConfig {
        enable_quality_processing: true,
        chunking: Some(crate::ChunkingConfig {
            max_characters: 500,
            overlap: 50,
            trim: true,
            chunker_type: crate::ChunkerType::Text,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    #[cfg(feature = "quality")]
    assert!(processed.quality_score.is_some());
    assert!(processed.chunks.is_some());
}

#[tokio::test]
#[serial]
#[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
async fn test_pipeline_with_keyword_extraction() {
    crate::plugins::registry::get_validator_registry()
        .write()
        .shutdown_all()
        .unwrap();
    crate::plugins::registry::get_post_processor_registry()
        .write()
        .shutdown_all()
        .unwrap();

    let _ = crate::keywords::register_keyword_processor();
    clear_processor_cache().unwrap();

    let doc = make_doc(
        r#"
Machine learning is a branch of artificial intelligence that focuses on
building systems that can learn from data. Deep learning is a subset of
machine learning that uses neural networks with multiple layers.
Natural language processing enables computers to understand human language.
            "#,
        "text/plain",
    );
    #[cfg(feature = "keywords-yake")]
    let keyword_config = crate::keywords::KeywordConfig::yake();

    #[cfg(all(feature = "keywords-rake", not(feature = "keywords-yake")))]
    let keyword_config = crate::keywords::KeywordConfig::rake();

    let config = ExtractionConfig {
        keywords: Some(keyword_config),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();

    let keywords = processed
        .extracted_keywords
        .as_ref()
        .expect("Should have extracted keywords");
    assert!(!keywords.is_empty(), "Should have extracted keywords");

    let first_keyword = &keywords[0];
    assert!(!first_keyword.text.is_empty());
    assert!(first_keyword.score > 0.0);
}

#[tokio::test]
#[serial]
#[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
async fn test_pipeline_without_keyword_config() {
    let doc = make_doc("Machine learning and artificial intelligence.", "text/plain");

    let config = ExtractionConfig {
        keywords: None,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();

    assert!(!processed.metadata.additional.contains_key("keywords"));
}

#[tokio::test]
#[serial]
#[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
async fn test_pipeline_keyword_extraction_short_content() {
    crate::plugins::registry::get_validator_registry()
        .write()
        .shutdown_all()
        .unwrap();
    crate::plugins::registry::get_post_processor_registry()
        .write()
        .shutdown_all()
        .unwrap();

    let doc = make_doc("Short text", "text/plain");

    #[cfg(feature = "keywords-yake")]
    let keyword_config = crate::keywords::KeywordConfig::yake();

    #[cfg(all(feature = "keywords-rake", not(feature = "keywords-yake")))]
    let keyword_config = crate::keywords::KeywordConfig::rake();

    let config = ExtractionConfig {
        keywords: Some(keyword_config),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();

    assert!(!processed.metadata.additional.contains_key("keywords"));
}

#[tokio::test]
#[serial]
async fn test_postprocessor_runs_before_validator() {
    use crate::plugins::{Plugin, PostProcessor, ProcessingStage, Validator};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct TestPostProcessor;
    impl Plugin for TestPostProcessor {
        fn name(&self) -> &str {
            "test-processor"
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
    impl PostProcessor for TestPostProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            result
                .metadata
                .additional
                .insert(Cow::Borrowed("processed"), serde_json::json!(true));
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Middle
        }
    }

    struct TestValidator;
    impl Plugin for TestValidator {
        fn name(&self) -> &str {
            "test-validator"
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
    impl Validator for TestValidator {
        async fn validate(&self, result: &ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            let should_validate = result
                .metadata
                .additional
                .get(VALIDATION_MARKER_KEY)
                .and_then(|v| v.as_str())
                == Some(POSTPROCESSOR_VALIDATION_MARKER);

            if !should_validate {
                return Ok(());
            }

            let processed = result
                .metadata
                .additional
                .get("processed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !processed {
                return Err(crate::XbergError::Validation {
                    message: "Post-processor did not run before validator".to_string(),
                    source: None,
                });
            }
            Ok(())
        }
    }

    let pp_registry = crate::plugins::registry::get_post_processor_registry();
    let val_registry = crate::plugins::registry::get_validator_registry();

    clear_processor_cache().unwrap();
    pp_registry.write().shutdown_all().unwrap();
    val_registry.write().shutdown_all().unwrap();
    clear_processor_cache().unwrap();

    {
        let mut registry = pp_registry.write();
        registry.register(Arc::new(TestPostProcessor)).unwrap();
    }

    {
        let mut registry = val_registry.write();
        registry.register(Arc::new(TestValidator)).unwrap();
    }

    clear_processor_cache().unwrap();

    let mut doc = make_doc("test", "text/plain");
    doc.metadata.additional.insert(
        Cow::Borrowed(VALIDATION_MARKER_KEY),
        serde_json::json!(POSTPROCESSOR_VALIDATION_MARKER),
    );

    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: true,
            enabled_set: None,
            disabled_set: None,
            enabled_processors: None,
            disabled_processors: None,
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await;

    pp_registry.write().shutdown_all().unwrap();
    val_registry.write().shutdown_all().unwrap();

    assert!(processed.is_ok(), "Validator should have seen post-processor metadata");
    let processed = processed.unwrap();
    assert_eq!(
        processed.metadata.additional.get("processed"),
        Some(&serde_json::json!(true)),
        "Post-processor metadata should be present"
    );
}

#[tokio::test]
#[serial]
#[cfg(feature = "quality")]
async fn test_quality_processing_runs_before_validator() {
    ensure_quality_processor();
    use crate::plugins::{Plugin, Validator};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct QualityValidator;
    impl Plugin for QualityValidator {
        fn name(&self) -> &str {
            "quality-validator"
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
    impl Validator for QualityValidator {
        async fn validate(&self, result: &ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            let should_validate = result
                .metadata
                .additional
                .get(VALIDATION_MARKER_KEY)
                .and_then(|v| v.as_str())
                == Some(QUALITY_VALIDATION_MARKER);

            if !should_validate {
                return Ok(());
            }

            if result.quality_score.is_none() {
                return Err(crate::XbergError::Validation {
                    message: "Quality processing did not run before validator".to_string(),
                    source: None,
                });
            }
            Ok(())
        }
    }

    let val_registry = crate::plugins::registry::get_validator_registry();
    {
        let mut registry = val_registry.write();
        registry.register(Arc::new(QualityValidator)).unwrap();
    }

    let mut doc = make_doc("This is meaningful test content for quality scoring.", "text/plain");
    doc.metadata.additional.insert(
        Cow::Borrowed(VALIDATION_MARKER_KEY),
        serde_json::json!(QUALITY_VALIDATION_MARKER),
    );

    let config = ExtractionConfig {
        enable_quality_processing: true,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await;

    {
        let mut registry = val_registry.write();
        registry.remove("quality-validator").unwrap();
    }

    assert!(processed.is_ok(), "Validator should have seen quality_score");
}

#[tokio::test]
#[serial]
async fn test_multiple_postprocessors_run_before_validator() {
    use crate::plugins::{Plugin, PostProcessor, ProcessingStage, Validator};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct EarlyProcessor;
    impl Plugin for EarlyProcessor {
        fn name(&self) -> &str {
            "early-proc"
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
    impl PostProcessor for EarlyProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            let mut order = result
                .metadata
                .additional
                .get("execution_order")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            order.push(serde_json::json!("early"));
            result
                .metadata
                .additional
                .insert(Cow::Borrowed("execution_order"), serde_json::json!(order));
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Early
        }
    }

    struct LateProcessor;
    impl Plugin for LateProcessor {
        fn name(&self) -> &str {
            "late-proc"
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
    impl PostProcessor for LateProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            let mut order = result
                .metadata
                .additional
                .get("execution_order")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            order.push(serde_json::json!("late"));
            result
                .metadata
                .additional
                .insert(Cow::Borrowed("execution_order"), serde_json::json!(order));
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Late
        }
    }

    struct OrderValidator;
    impl Plugin for OrderValidator {
        fn name(&self) -> &str {
            "order-validator"
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
    impl Validator for OrderValidator {
        async fn validate(&self, result: &ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            let should_validate = result
                .metadata
                .additional
                .get(VALIDATION_MARKER_KEY)
                .and_then(|v| v.as_str())
                == Some(ORDER_VALIDATION_MARKER);

            if !should_validate {
                return Ok(());
            }

            let order = result
                .metadata
                .additional
                .get("execution_order")
                .and_then(|v| v.as_array())
                .ok_or_else(|| crate::XbergError::Validation {
                    message: "No execution order found".to_string(),
                    source: None,
                })?;

            if order.len() != 2 {
                return Err(crate::XbergError::Validation {
                    message: format!("Expected 2 processors to run, got {}", order.len()),
                    source: None,
                });
            }

            if order[0] != "early" || order[1] != "late" {
                return Err(crate::XbergError::Validation {
                    message: format!("Wrong execution order: {:?}", order),
                    source: None,
                });
            }

            Ok(())
        }
    }

    let pp_registry = crate::plugins::registry::get_post_processor_registry();
    let val_registry = crate::plugins::registry::get_validator_registry();

    pp_registry.write().shutdown_all().unwrap();
    val_registry.write().shutdown_all().unwrap();
    clear_processor_cache().unwrap();

    {
        let mut registry = pp_registry.write();
        registry.register(Arc::new(EarlyProcessor)).unwrap();
        registry.register(Arc::new(LateProcessor)).unwrap();
    }

    {
        let mut registry = val_registry.write();
        registry.register(Arc::new(OrderValidator)).unwrap();
    }

    clear_processor_cache().unwrap();

    let doc = make_doc("test", "text/plain");

    let config = ExtractionConfig::default();

    let processed = run_pipeline(doc, &config).await;

    pp_registry.write().shutdown_all().unwrap();
    val_registry.write().shutdown_all().unwrap();
    clear_processor_cache().unwrap();

    assert!(processed.is_ok(), "All processors should run before validator");
}

#[tokio::test]
#[serial]
#[cfg(feature = "chunking")]
async fn test_middle_postprocessors_run_after_explicit_chunking() {
    use crate::plugins::{Plugin, PostProcessor, ProcessingStage};
    use async_trait::async_trait;
    use std::sync::Arc;

    const CHUNK_MARKER: &str = "middle_saw_chunks";

    struct ChunkAwareMiddleProcessor;
    impl Plugin for ChunkAwareMiddleProcessor {
        fn name(&self) -> &str {
            "chunk-aware-middle"
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
    impl PostProcessor for ChunkAwareMiddleProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            result.metadata.additional.insert(
                Cow::Borrowed(CHUNK_MARKER),
                serde_json::json!(result.chunks.as_ref().is_some_and(|chunks| !chunks.is_empty())),
            );
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Middle
        }
    }

    let registry = crate::plugins::registry::get_post_processor_registry();
    {
        let mut reg = registry.write();
        reg.register(Arc::new(ChunkAwareMiddleProcessor)).unwrap();
    }
    clear_processor_cache().unwrap();

    let doc = make_doc(&"chunk me ".repeat(100), "text/plain");
    let config = ExtractionConfig {
        chunking: Some(crate::ChunkingConfig {
            max_characters: 80,
            overlap: 0,
            trim: true,
            chunker_type: crate::ChunkerType::Text,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await;

    {
        let mut reg = registry.write();
        reg.remove("chunk-aware-middle").unwrap();
    }
    clear_processor_cache().unwrap();

    let processed = processed.unwrap();
    assert_eq!(
        processed.metadata.additional.get(CHUNK_MARKER),
        Some(&serde_json::json!(true)),
        "Middle-stage processors should see explicit chunking output"
    );
}

#[tokio::test]
#[serial]
#[cfg(all(feature = "captioning", feature = "redaction", feature = "chunking"))]
async fn captioning_prepass_keeps_redaction_and_chunks_consistent() {
    use crate::core::config::{CaptioningConfig, LlmConfig, RedactionConfig};
    use crate::plugins::{Plugin, PostProcessor, ProcessingStage};
    use crate::types::ExtractedImage;
    use async_trait::async_trait;
    use bytes::Bytes;
    use std::sync::Arc;

    const ORIGINAL_PII: &str = "alice@example.com";
    const CAPTION_PREFIX: &str = "Photo owned by";

    struct StubCaptioningProcessor;

    impl Plugin for StubCaptioningProcessor {
        fn name(&self) -> &str {
            CAPTIONING_PROCESSOR_NAME
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
    impl PostProcessor for StubCaptioningProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            for image in result.images.iter_mut().flatten() {
                let caption = format!("{CAPTION_PREFIX} {ORIGINAL_PII}");
                image.description = Some(caption.clone());
                image.caption = Some(caption);
            }
            result.processing_warnings.push(crate::types::ProcessingWarning {
                source: Cow::Borrowed("captioning"),
                message: Cow::Borrowed("synthetic caption warning"),
            });
            result.llm_usage.get_or_insert_default().push(crate::types::LlmUsage {
                model: "synthetic-caption-model".to_string(),
                source: "captioning".to_string(),
                ..Default::default()
            });
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Middle
        }

        fn priority(&self) -> i32 {
            50
        }
    }

    initialize_features();
    crate::plugins::processor::builtin::redaction::register().unwrap();
    let registry = crate::plugins::registry::get_post_processor_registry();
    registry.write().register(Arc::new(StubCaptioningProcessor)).unwrap();
    clear_processor_cache().unwrap();

    let mut doc = make_doc(&format!("Contact {ORIGINAL_PII}."), "application/pdf");
    doc.processing_warnings.push(crate::types::ProcessingWarning {
        source: Cow::Borrowed("extraction"),
        message: Cow::Borrowed("synthetic extraction warning"),
    });
    doc.llm_usage = Some(vec![crate::types::LlmUsage {
        model: "synthetic-extraction-model".to_string(),
        source: "extraction".to_string(),
        ..Default::default()
    }]);
    let image_index = doc.push_image(ExtractedImage {
        data: Bytes::from_static(b"synthetic image"),
        format: Cow::Borrowed("png"),
        image_index: 0,
        width: Some(100),
        height: Some(100),
        ..Default::default()
    });
    doc.push_element(InternalElement::text(ElementKind::Image { image_index }, "", 0));

    let config = ExtractionConfig {
        captioning: Some(CaptioningConfig {
            llm: LlmConfig::default(),
            prompt: None,
            min_image_area: 1,
        }),
        redaction: Some(RedactionConfig::default()),
        chunking: Some(crate::ChunkingConfig {
            max_characters: 500,
            overlap: 0,
            ..Default::default()
        }),
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await;

    crate::plugins::processor::builtin::captioning::register().unwrap();
    clear_processor_cache().unwrap();

    let processed = processed.unwrap();
    assert!(
        processed.content.contains(CAPTION_PREFIX),
        "redacted output must retain the generated caption: {}",
        processed.content
    );
    assert!(
        !processed.content.contains(ORIGINAL_PII),
        "redacted output must not restore original PII: {}",
        processed.content
    );
    if let Some(formatted_content) = processed.formatted_content.as_deref() {
        assert!(
            formatted_content.contains(CAPTION_PREFIX),
            "formatted output must retain the generated caption: {formatted_content}"
        );
        assert!(
            !formatted_content.contains(ORIGINAL_PII),
            "formatted output must not restore original PII: {formatted_content}"
        );
    }

    let caption = processed
        .images
        .as_deref()
        .and_then(|images| images.first())
        .and_then(|image| image.caption.as_deref())
        .expect("captioning must retain the image caption");
    assert!(caption.contains(CAPTION_PREFIX), "image caption was lost: {caption}");
    assert!(
        !caption.contains(ORIGINAL_PII),
        "image caption must be redacted: {caption}"
    );

    let chunk_text = processed
        .chunks
        .as_ref()
        .expect("chunking must produce chunks")
        .iter()
        .map(|chunk| chunk.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        chunk_text.contains(CAPTION_PREFIX),
        "chunks must include the generated caption: {chunk_text}"
    );
    assert!(
        !chunk_text.contains(ORIGINAL_PII),
        "redacted chunks must not contain original PII: {chunk_text}"
    );
    assert_eq!(
        processed
            .processing_warnings
            .iter()
            .map(|warning| warning.source.as_ref())
            .collect::<Vec<_>>(),
        vec!["extraction", "captioning"],
        "captioning prepass must preserve existing warnings and append its own"
    );
    assert_eq!(
        processed
            .llm_usage
            .as_ref()
            .expect("prepass must preserve LLM usage")
            .iter()
            .map(|usage| usage.source.as_str())
            .collect::<Vec<_>>(),
        vec!["extraction", "captioning"],
        "captioning prepass must preserve existing usage and append its own"
    );
}

/// #355: the captioning prepass derives a full `ExtractedDocument` from `doc`
/// before the pipeline's main (second) derivation runs on the same `doc`. That
/// first derivation destructively `.remove()`s `CODE_INTELLIGENCE_SCRATCH_KEY`
/// from `metadata.additional` (see `derive::derive_extraction_result`), and the
/// prepass then overwrites `doc.metadata` wholesale with the already-stripped
/// copy. Without carrying the computed payload back onto `doc`, the second
/// derivation finds no scratch key and silently falls back to a degraded
/// `CodeMetadata`-only `code_intelligence` (losing metrics, structure, imports,
/// exports, etc. — see #259). Assert the exact full payload survives.
#[tokio::test]
#[serial]
#[cfg(all(feature = "captioning", feature = "tree-sitter"))]
async fn captioning_prepass_preserves_full_code_intelligence_scratch_payload() {
    use crate::core::config::{CaptioningConfig, LlmConfig};
    use crate::plugins::{Plugin, PostProcessor, ProcessingStage};
    use crate::types::metadata::{CodeMetadata, FormatMetadata};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct NoopCaptioningProcessor;

    impl Plugin for NoopCaptioningProcessor {
        fn name(&self) -> &str {
            CAPTIONING_PROCESSOR_NAME
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
    impl PostProcessor for NoopCaptioningProcessor {
        async fn process(&self, _result: &mut ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Middle
        }

        fn priority(&self) -> i32 {
            50
        }
    }

    initialize_features();
    let registry = crate::plugins::registry::get_post_processor_registry();
    registry.write().register(Arc::new(NoopCaptioningProcessor)).unwrap();
    clear_processor_cache().unwrap();

    let expected_code_intelligence = serde_json::json!({
        "language": "python",
        "metrics": {"total_lines": 1},
    });

    let mut doc = make_doc("def f(): pass", "text/x-python");
    doc.metadata.format = Some(FormatMetadata::Code(CodeMetadata::default()));
    doc.metadata.additional.insert(
        Cow::Borrowed(crate::extractors::code::CODE_INTELLIGENCE_SCRATCH_KEY),
        expected_code_intelligence.clone(),
    );

    let config = ExtractionConfig {
        captioning: Some(CaptioningConfig {
            llm: LlmConfig::default(),
            prompt: None,
            min_image_area: 1,
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();

    // Restore the real captioning processor for subsequent tests in this module.
    crate::plugins::processor::builtin::captioning::register().unwrap();
    clear_processor_cache().unwrap();

    assert_eq!(
        processed.code_intelligence,
        Some(expected_code_intelligence),
        "captioning prepass must not degrade code_intelligence on the pipeline's second derivation"
    );
    assert!(
        !processed
            .metadata
            .additional
            .contains_key(crate::extractors::code::CODE_INTELLIGENCE_SCRATCH_KEY),
        "scratch key must never leak into final metadata"
    );
}

#[tokio::test]
#[serial]
async fn test_run_pipeline_with_output_format_plain() {
    let doc = make_doc("test content", "text/plain");

    let config = crate::core::config::ExtractionConfig {
        output_format: OutputFormat::Plain,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.content, "test content");
    assert_eq!(processed.metadata.output_format, Some("plain".to_string()));
}

#[tokio::test]
#[serial]
async fn test_run_pipeline_with_output_format_djot() {
    let doc = make_doc("test content", "text/djot");

    let config = crate::core::config::ExtractionConfig {
        output_format: OutputFormat::Djot,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(!processed.content.is_empty());
    assert_eq!(processed.metadata.output_format, Some("djot".to_string()));
}

#[tokio::test]
#[serial]
async fn test_run_pipeline_with_output_format_html() {
    let doc = make_doc("test content", "text/plain");

    let config = crate::core::config::ExtractionConfig {
        output_format: OutputFormat::Html,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(processed.content.contains("test content"));
    assert_eq!(processed.metadata.output_format, Some("html".to_string()));
}

#[tokio::test]
#[serial]
#[cfg(feature = "quality")]
async fn test_nfc_normalization_decomposes_to_composed() {
    let doc = make_doc("caf\u{0065}\u{0301}", "text/plain");
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.content, "caf\u{00e9}");
    assert!(!processed.content.contains('\u{0301}'));
}

#[tokio::test]
#[serial]
#[cfg(feature = "quality")]
async fn test_nfc_normalization_idempotent_on_ascii() {
    let doc = make_doc("Hello, world! 123", "text/plain");
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.content, "Hello, world! 123");
}

#[tokio::test]
#[serial]
#[cfg(feature = "quality")]
async fn test_nfc_normalization_applies_to_page_content() {
    let mut doc = InternalDocument::new("plain");
    doc.mime_type = "text/plain".to_string();
    doc.push_element(InternalElement::text(ElementKind::Paragraph, "re\u{0301}sume\u{0301}", 0).with_page(1));
    let config = ExtractionConfig {
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert!(processed.content.contains("r\u{00e9}sum\u{00e9}"));
    let pages = processed.pages.unwrap();
    assert_eq!(pages[0].content, "r\u{00e9}sum\u{00e9}");
}

#[tokio::test]
#[serial]
async fn test_run_pipeline_applies_output_format_last() {
    let doc = make_doc("test", "text/plain");

    let config = crate::core::config::ExtractionConfig {
        output_format: OutputFormat::Djot,
        enable_quality_processing: false,
        ..Default::default()
    };

    let processed = run_pipeline(doc, &config).await.unwrap();
    assert_eq!(processed.metadata.output_format, Some("djot".to_string()));
}

#[tokio::test]
#[serial]
#[cfg(all(feature = "pdf", feature = "chunking"))]
async fn test_chunking_populates_page_numbers_for_pdf() {
    use crate::core::config::ChunkingConfig;

    let pdf_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/pdf/issue-636-chunk-pages.pdf");

    if !pdf_path.exists() {
        return;
    }

    let pdf_bytes = std::fs::read(&pdf_path).unwrap();

    let config = ExtractionConfig {
        chunking: Some(ChunkingConfig {
            max_characters: 500,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = crate::core::extractor::extract_bytes(&pdf_bytes, "application/pdf", &config)
        .await
        .unwrap();

    assert!(result.chunks.is_some(), "Chunks should be produced");
    let chunks = result.chunks.as_ref().unwrap();
    assert!(!chunks.is_empty(), "Should have at least one chunk");

    let chunks_with_pages = chunks.iter().filter(|c| c.metadata.first_page.is_some()).count();
    assert!(
        chunks_with_pages > 0,
        "At least some chunks should have page numbers, but none do. Total chunks: {}",
        chunks.len()
    );
}

#[tokio::test]
#[serial]
#[cfg(feature = "chunking")]
async fn test_pipeline_chunks_content_matches_output_format_markdown() {
    use crate::core::config::{ChunkerType, ChunkingConfig};
    use crate::types::internal::ElementKind;

    let mut doc = InternalDocument::new("plain");
    doc.mime_type = "text/plain".to_string();
    doc.push_element(InternalElement::text(ElementKind::Heading { level: 1 }, "Section", 0));
    doc.push_element(InternalElement::text(
        ElementKind::Paragraph,
        "Body text for the section. ".repeat(10),
        0,
    ));

    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        chunking: Some(ChunkingConfig {
            max_characters: 200,
            overlap: 0,
            trim: true,
            chunker_type: ChunkerType::Markdown,
            ..Default::default()
        }),
        postprocessor: Some(crate::core::config::PostProcessorConfig {
            enabled: false,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = run_pipeline(doc, &config).await.unwrap();

    assert_eq!(result.metadata.output_format, Some("markdown".to_string()));
    assert!(
        result.content.contains('#'),
        "top-level content must contain markdown heading, got: {:?}",
        &result.content[..result.content.len().min(120)]
    );

    let chunks = result.chunks.expect("chunks must be produced");
    assert!(!chunks.is_empty(), "at least one chunk must be produced");
    let all_chunk_content: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join("\n");
    assert!(
        all_chunk_content.contains('#'),
        "chunks[].content must contain markdown syntax, got: {:?}",
        &all_chunk_content[..all_chunk_content.len().min(200)]
    );
}

#[test]
fn test_append_ocr_text_for_pptx_images() {
    use crate::types::ExtractedImage;
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
    use std::borrow::Cow;

    let mut doc = InternalDocument::new("pptx");
    doc.append_ocr_text = true;
    doc.elements
        .push(InternalElement::text(ElementKind::Paragraph, "Before image.", 0));
    doc.elements.push(InternalElement::text(
        ElementKind::Paragraph,
        "![img](../media/image-1.jpeg)",
        0,
    ));
    doc.elements
        .push(InternalElement::text(ElementKind::Paragraph, "After image.", 0));

    doc.images.push(ExtractedImage {
        data: bytes::Bytes::new(),
        format: Cow::Borrowed("jpeg"),
        image_index: 0,
        page_number: Some(1),
        width: Some(100),
        height: Some(100),
        colorspace: None,
        bits_per_component: None,
        is_mask: false,
        description: None,
        ocr_result: Some(Box::new(crate::types::ExtractedDocument {
            content: "OCR text here".to_string(),
            mime_type: Cow::Borrowed("text/plain"),
            ..Default::default()
        })),
        bounding_box: None,
        source_path: None,
        image_kind: None,
        kind_confidence: None,
        cluster_id: None,
        caption: None,
        qr_codes: None,
        data_base64: None,
    });

    super::append_embedded_image_ocr_text(&mut doc);

    assert_eq!(
        doc.elements.len(),
        4,
        "should have 4 elements (original 3 + 1 OCR paragraph)"
    );
    assert_eq!(doc.elements[2].text, "OCR text here");

    let rendered = crate::rendering::render_markdown(&doc);
    assert!(rendered.contains("OCR text here"));
}

/// Smoke tests for `apply_output_format_pass`.
///
/// These operate directly on `ExtractedDocument` without invoking the full extractor,
/// proving the pass executes correctly when called at the pipeline level.
#[cfg(feature = "image-encode")]
mod output_format_pass_tests {
    use std::borrow::Cow;
    use std::io::Cursor;

    use bytes::Bytes;
    use image::{DynamicImage, ImageFormat};

    use crate::core::config::extraction::{ImageExtractionConfig, ImageOutputFormat};
    use crate::types::{ExtractedDocument, ExtractedImage};

    use super::apply_output_format_pass;

    fn make_jpeg_bytes() -> Bytes {
        use image::codecs::jpeg::JpegEncoder;
        let img = image::RgbImage::new(8, 8);
        let mut buf: Vec<u8> = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 85)
            .encode_image(&DynamicImage::ImageRgb8(img))
            .expect("test JPEG encode");
        Bytes::from(buf)
    }

    fn make_png_bytes() -> Bytes {
        let img = image::RgbImage::new(8, 8);
        let mut buf: Vec<u8> = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .expect("test PNG encode");
        Bytes::from(buf)
    }

    fn make_image(data: Bytes, format: &'static str) -> ExtractedImage {
        ExtractedImage {
            data,
            format: Cow::Borrowed(format),
            ..Default::default()
        }
    }

    /// Both decodable images are re-encoded to PNG; no warnings are pushed.
    #[test]
    fn both_images_re_encoded_to_png_no_warnings() {
        let mut result = ExtractedDocument {
            images: Some(vec![
                make_image(make_jpeg_bytes(), "jpeg"),
                make_image(make_png_bytes(), "png"),
            ]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            output_format: ImageOutputFormat::Png,
            ..Default::default()
        };

        apply_output_format_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        assert_eq!(images[0].format.as_ref(), "png", "jpeg must be re-encoded to png");
        assert_eq!(images[1].format.as_ref(), "png", "already-png must remain png");
        assert!(
            result.processing_warnings.is_empty(),
            "no warnings expected for decodable images; got: {:?}",
            result.processing_warnings
        );
    }

    /// Without the `svg` feature: an SVG image is left untouched and a
    /// `ProcessingWarning` is pushed for it (it is an untranslatable format).
    #[cfg(not(feature = "svg"))]
    #[test]
    fn svg_image_skipped_with_warning() {
        let svg_bytes = Bytes::from_static(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        let original_svg = svg_bytes.clone();

        let mut result = ExtractedDocument {
            images: Some(vec![
                make_image(make_jpeg_bytes(), "jpeg"),
                make_image(svg_bytes, "svg"),
            ]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            output_format: ImageOutputFormat::Png,
            ..Default::default()
        };

        apply_output_format_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        assert_eq!(images[0].format.as_ref(), "png", "jpeg must be re-encoded");
        assert_eq!(images[1].format.as_ref(), "svg", "svg must be untouched");
        assert_eq!(images[1].data, original_svg, "svg bytes must be untouched");

        assert_eq!(result.processing_warnings.len(), 1, "one warning for svg");
        assert_eq!(
            result.processing_warnings[0].source.as_ref(),
            "image_encoder",
            "warning source must be image_encoder"
        );
    }

    /// With the `svg` feature: an SVG image is rasterized to the target format
    /// (PNG here) via `resvg`/`usvg`.  No warning is pushed — the encode succeeds.
    #[cfg(feature = "svg")]
    #[test]
    fn svg_image_skipped_with_warning() {
        let svg_bytes = Bytes::from_static(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>");

        let mut result = ExtractedDocument {
            images: Some(vec![
                make_image(make_jpeg_bytes(), "jpeg"),
                make_image(svg_bytes, "svg"),
            ]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            output_format: ImageOutputFormat::Png,
            ..Default::default()
        };

        apply_output_format_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        assert_eq!(images[0].format.as_ref(), "png", "jpeg must be re-encoded to png");
        assert_eq!(images[1].format.as_ref(), "png", "svg must be rasterized to png");
        assert!(
            result.processing_warnings.is_empty(),
            "no warnings expected when svg is rasterized successfully; got: {:?}",
            result.processing_warnings
        );
    }

    /// When output_format is Native the pass is a no-op.
    #[test]
    fn native_target_is_no_op() {
        let original = make_jpeg_bytes();
        let mut result = ExtractedDocument {
            images: Some(vec![make_image(original.clone(), "jpeg")]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            output_format: ImageOutputFormat::Native,
            ..Default::default()
        };

        apply_output_format_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        assert_eq!(images[0].data, original, "bytes must be untouched for Native");
        assert_eq!(images[0].format.as_ref(), "jpeg", "format must be untouched");
        assert!(result.processing_warnings.is_empty());
    }
}

/// Unit tests for `apply_data_base64_pass`.
///
/// Directly exercises the private pass without going through the full extractor,
/// mirroring the approach used by `output_format_pass_tests` above.
mod data_base64_pass_tests {
    use std::borrow::Cow;

    use base64::Engine as _;
    use bytes::Bytes;

    use crate::core::config::extraction::ImageExtractionConfig;
    use crate::types::{ExtractedDocument, ExtractedImage};

    use super::apply_data_base64_pass;

    fn make_image(data: Bytes) -> ExtractedImage {
        ExtractedImage {
            data,
            format: Cow::Borrowed("png"),
            ..Default::default()
        }
    }

    /// When `include_data_base64` is `true` every image's `data_base64` must be
    /// `Some(base64::STANDARD.encode(image.data))`.
    #[test]
    fn include_data_base64_true_encodes_all_images() {
        let first_bytes = Bytes::from_static(b"\x89PNG\r\n\x1a\n");
        let second_bytes = Bytes::from_static(b"\xff\xd8\xff");

        let mut result = ExtractedDocument {
            images: Some(vec![make_image(first_bytes.clone()), make_image(second_bytes.clone())]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            include_data_base64: true,
            ..Default::default()
        };

        apply_data_base64_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        assert_eq!(
            images[0].data_base64,
            Some(base64::engine::general_purpose::STANDARD.encode(&first_bytes)),
            "first image data_base64 must be the STANDARD-encoded bytes"
        );
        assert_eq!(
            images[1].data_base64,
            Some(base64::engine::general_purpose::STANDARD.encode(&second_bytes)),
            "second image data_base64 must be the STANDARD-encoded bytes"
        );
    }

    /// When `include_data_base64` is `false` (the default) no image must have
    /// its `data_base64` field populated.
    #[test]
    fn include_data_base64_false_leaves_field_none() {
        let mut result = ExtractedDocument {
            images: Some(vec![
                make_image(Bytes::from_static(b"\x89PNG\r\n\x1a\n")),
                make_image(Bytes::from_static(b"\xff\xd8\xff")),
            ]),
            ..Default::default()
        };

        let cfg = ImageExtractionConfig {
            include_data_base64: false,
            ..Default::default()
        };

        apply_data_base64_pass(&mut result, &cfg);

        let images = result.images.as_ref().expect("images must be present");
        for (idx, image) in images.iter().enumerate() {
            assert_eq!(
                image.data_base64, None,
                "image[{idx}].data_base64 must remain None when include_data_base64 is false"
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn test_pdf_run_fallback_not_suppressed_without_images_config() {
    use crate::core::config::ImageExtractionConfig;

    let default_no_images = crate::core::config::ExtractionConfig::default();
    assert!(
        default_no_images.images.is_none(),
        "baseline: default config has no images section"
    );

    let skip_fallback = default_no_images
        .images
        .as_ref()
        .map(|i| i.run_ocr_on_images)
        .unwrap_or(false);
    assert!(
        !skip_fallback,
        "RunFallback must NOT be suppressed when config.images is None"
    );

    let with_images_opted_in = crate::core::config::ExtractionConfig {
        images: Some(ImageExtractionConfig {
            run_ocr_on_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let skip_fallback_opted_in = with_images_opted_in
        .images
        .as_ref()
        .map(|i| i.run_ocr_on_images)
        .unwrap_or(false);
    assert!(
        skip_fallback_opted_in,
        "RunFallback must be suppressed when images.run_ocr_on_images=true"
    );
}

mod document_counts {
    use super::super::populate_document_counts;
    use crate::types::page::{PageContent, PageStructure, PageUnitType};
    use crate::types::{ExtractedDocument, ExtractedImage, Metadata, Table};

    fn page_structure(total_count: u32) -> PageStructure {
        PageStructure {
            total_count,
            unit_type: PageUnitType::Page,
            boundaries: None,
            pages: None,
        }
    }

    fn page(page_number: u32) -> PageContent {
        PageContent {
            page_number,
            content: String::new(),
            tables: Vec::new(),
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        }
    }

    #[test]
    fn pages_come_from_metadata_page_count() {
        let mut result = ExtractedDocument {
            metadata: Metadata {
                pages: Some(page_structure(5)),
                ..Default::default()
            },
            tables: vec![Table::default(), Table::default()],
            images: Some(vec![ExtractedImage::default()]),
            pages: None,
            ..Default::default()
        };
        populate_document_counts(&mut result);
        assert_eq!(result.counts.pages, 5, "pages must read metadata.total_count");
        assert_eq!(result.counts.tables, 2);
        assert_eq!(result.counts.images, 1);
    }

    #[test]
    fn pages_fall_back_to_materialized_pages_len() {
        let mut result = ExtractedDocument {
            metadata: Metadata::default(),
            pages: Some(vec![page(1), page(2), page(3)]),
            ..Default::default()
        };
        populate_document_counts(&mut result);
        assert_eq!(result.counts.pages, 3);
        assert_eq!(result.counts.tables, 0);
        assert_eq!(result.counts.images, 0);
    }

    #[test]
    fn non_paginated_input_reports_zero_pages() {
        let mut result = ExtractedDocument {
            content: "plain text".to_string(),
            ..Default::default()
        };
        populate_document_counts(&mut result);
        assert_eq!(result.counts.pages, 0);
        assert_eq!(result.counts.tables, 0);
        assert_eq!(result.counts.images, 0);
    }

    #[test]
    fn zero_metadata_page_count_falls_back_to_pages_len() {
        let mut result = ExtractedDocument {
            metadata: Metadata {
                pages: Some(page_structure(0)),
                ..Default::default()
            },
            pages: Some(vec![page(1), page(2)]),
            ..Default::default()
        };
        populate_document_counts(&mut result);
        assert_eq!(result.counts.pages, 2);
    }
}
