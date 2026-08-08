#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]
// ~keep: test/bench binaries print by design; org logging policy exempts tests
// `tokio::spawn`ing an extraction future requires proving it `Send`. Under `full`
// features that proof walks into h2's `PollMessage` -> `slab::Entry` -> `PhantomData`
// chain (reqwest/hyper, pulled in by api/mcp/liter-llm), which exceeds the default
// 128 limit. The chain is entirely third-party — no xberg type appears in it — and
// rustc's own diagnostic recommends exactly this. ~keep
#![recursion_limit = "256"]
//! Comprehensive concurrency and parallelism stress tests.
//!
//! Validates that the Xberg core handles concurrent operations correctly:
//! - Parallel extractions don't interfere with each other
//! - OCR processing is thread-safe and efficient
//! - Pipeline processing works correctly under concurrent load
//! - Cache access is safe with multiple readers/writers
//! - Registry access is thread-safe
//!
//! These tests ensure production workloads with high concurrency work correctly.

use async_trait::async_trait;
use std::sync::Arc;
use xberg::Result;
use xberg::core::config::{ExtractionConfig, PostProcessorConfig};
use xberg::core::pipeline::run_pipeline;
use xberg::internal::{ElementKind, InternalDocument, InternalElement};
use xberg::plugins::registry::{get_document_extractor_registry, get_post_processor_registry};
use xberg::plugins::{Plugin, PostProcessor, ProcessingStage};
use xberg::types::ExtractedDocument;

#[cfg(feature = "ocr")]
use xberg::core::config::OcrConfig;

use std::time::Duration;
use tokio::time::timeout;

mod helpers;
use helpers::{BytesInput, extract_bytes_document, extract_bytes_documents};

fn trim_trailing_newlines(value: &str) -> &str {
    value.trim_end_matches(['\n', '\r'])
}

fn assert_text_content(actual: &str, expected: &str) {
    assert_eq!(
        trim_trailing_newlines(actual),
        expected,
        "Content mismatch after trimming trailing newlines"
    );
}

/// Test many concurrent extractions of different MIME types.
///
/// Validates that:
/// - Registry lookups don't block each other unnecessarily
/// - Different extractors can run in parallel
/// - No data races or corruption
#[tokio::test]
async fn test_concurrent_extractions_mixed_formats() {
    let config = ExtractionConfig::default();

    #[allow(unused_mut)]
    let mut test_cases = vec![
        (b"Plain text content" as &[u8], "text/plain"),
        (b"{\"key\": \"value\"}", "application/json"),
        (b"# Markdown\n\nContent here", "text/markdown"),
    ];

    #[cfg(feature = "xml")]
    test_cases.push((b"<root><item>XML content</item></root>" as &[u8], "application/xml"));

    let mut handles = vec![];
    for _ in 0..10 {
        for (data, mime_type) in &test_cases {
            let config = config.clone();
            let data = data.to_vec();
            let mime_type = mime_type.to_string();

            handles.push(tokio::spawn(async move {
                extract_bytes_document(&data, &mime_type, &config).await
            }));
        }
    }

    let results = timeout(Duration::from_secs(30), async {
        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.expect("Task should not panic"));
        }
        results
    })
    .await
    .expect("All extractions should complete within 30s");

    for result in results {
        assert!(
            result.is_ok(),
            "Concurrent extraction should succeed: {:?}",
            result.err()
        );
    }
}

/// Test concurrent batch extractions.
///
/// Validates that batch processing correctly handles parallelism internally.
#[tokio::test]
async fn test_concurrent_batch_extractions() {
    let config = ExtractionConfig::default();

    let contents: Vec<Vec<u8>> = (0..20).map(|i| format!("Content {}", i).into_bytes()).collect();

    let mut handles = vec![];
    for _ in 0..5 {
        let config = config.clone();
        let contents_clone = contents.clone();

        handles.push(tokio::spawn(async move {
            let owned_data: Vec<BytesInput> = contents_clone
                .iter()
                .map(|c| BytesInput {
                    content: c.to_vec(),
                    mime_type: "text/plain".to_string(),
                    config: None,
                })
                .collect();
            extract_bytes_documents(owned_data, &config).await
        }));
    }

    for handle in handles {
        let results = handle.await.expect("Task should not panic");
        assert!(results.is_ok(), "Batch extraction should succeed");
        let results = results.expect("Operation failed");
        assert_eq!(results.len(), 20, "Should return all results");
    }
}

/// Test concurrent extractions with caching enabled.
///
/// Validates that:
/// - Cache reads/writes are thread-safe
/// - No cache corruption under concurrent access
/// - Cache hits work correctly across threads
#[tokio::test]
async fn test_concurrent_extractions_with_cache() {
    let config = ExtractionConfig {
        use_cache: true,
        postprocessor: Some(PostProcessorConfig {
            enabled: false,
            enabled_processors: None,
            disabled_processors: None,
            enabled_set: None,
            disabled_set: None,
        }),
        ..Default::default()
    };

    let test_data = b"Cached content for concurrent access test";

    let _ = extract_bytes_document(test_data, "text/plain", &config)
        .await
        .expect("Async operation failed");

    let mut handles = vec![];
    for _ in 0..100 {
        let config = config.clone();
        let data = test_data.to_vec();

        handles.push(tokio::spawn(async move {
            extract_bytes_document(&data, "text/plain", &config).await
        }));
    }

    let expected_content = "Cached content for concurrent access test";
    for handle in handles {
        let result = handle.await.expect("Task should not panic");
        assert!(result.is_ok(), "Cache read should succeed");
        let extraction = result.expect("Operation failed");
        assert_text_content(&extraction.content, expected_content);
    }
}

/// Test concurrent OCR processing of different images.
///
/// Validates that:
/// - OCR backend is thread-safe
/// - Multiple OCR operations don't interfere
/// - OCR cache handles concurrent access correctly
#[cfg(feature = "ocr")]
#[tokio::test]
async fn test_concurrent_ocr_processing() {
    use helpers::{get_test_file_path, skip_if_missing};

    if cfg!(windows) {
        return;
    }

    if skip_if_missing("images/ocr_image.jpg") {
        tracing::debug!("Skipping concurrent OCR test: test file not available");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: false,
        use_cache: true,
        ..Default::default()
    };

    let file_path = get_test_file_path("images/ocr_image.jpg");

    let mut handles = vec![];
    for _ in 0..20 {
        let file_path = file_path.clone();
        let config = config.clone();

        handles.push(tokio::task::spawn_blocking(move || {
            helpers::extract_uri_document_blocking(&file_path, None, &config)
        }));
    }

    let results = timeout(Duration::from_secs(60), async {
        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.expect("Task should not panic"));
        }
        results
    })
    .await
    .expect("All OCR operations should complete within 60s");

    let mut extracted_texts = vec![];
    for result in results {
        assert!(result.is_ok(), "OCR should succeed: {:?}", result.err());
        let extraction = result.expect("Operation failed");
        assert!(!extraction.content.is_empty(), "OCR should extract text");
        extracted_texts.push(extraction.content);
    }

    let first_text = &extracted_texts[0];
    for text in &extracted_texts[1..] {
        assert_eq!(text, first_text, "Concurrent OCR should produce identical results");
    }
}

/// Test concurrent OCR with cache warming.
///
/// Validates cache performance under concurrent load.
///
/// Note: This test is simplified to avoid runtime nesting issues.
/// It validates that concurrent OCR extractions work correctly with caching.
///
/// WARNING: This test uses timing heuristics (<500ms = cache hit) which are unreliable
/// in CI environments where even cached operations may exceed the threshold on slow runners.
/// Ignored to prevent flaky failures - cache hit rates vary significantly across platforms.
#[cfg(feature = "ocr")]
#[ignore = "flaky timing-based cache heuristic - cache hit rates vary significantly across platforms"]
#[test]
fn test_concurrent_ocr_cache_stress() {
    use helpers::{get_test_file_path, skip_if_missing};
    use std::sync::atomic::{AtomicUsize, Ordering};

    if skip_if_missing("images/ocr_image.jpg") {
        tracing::debug!("Skipping OCR cache stress test: test file not available");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: false,
        use_cache: true,
        ..Default::default()
    };

    let file_path = get_test_file_path("images/ocr_image.jpg");

    let first_result = helpers::extract_uri_document_blocking(&file_path, None, &config);
    assert!(first_result.is_ok(), "Initial OCR should succeed");

    let cache_hit_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];
    for _ in 0..50 {
        let file_path = file_path.clone();
        let config = config.clone();
        let hit_count = Arc::clone(&cache_hit_count);

        handles.push(std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let result = helpers::extract_uri_document_blocking(&file_path, None, &config);
            let duration = start.elapsed();

            if duration < Duration::from_millis(500) {
                hit_count.fetch_add(1, Ordering::Relaxed);
            }

            result
        }));
    }

    for handle in handles {
        let result = handle.join().expect("Thread should not panic");
        assert!(result.is_ok(), "Cached OCR should succeed");
    }

    let hits = cache_hit_count.load(Ordering::Relaxed);
    assert!(
        hits >= 20,
        "At least 20/50 requests should hit cache, got {} hits",
        hits
    );
}

/// Test concurrent pipeline processing.
///
/// Validates that:
/// - Pipeline can process multiple results in parallel
/// - Processors don't interfere with each other
/// - Registry reads are thread-safe
///
/// Note: This test is flaky due to timing-dependent concurrent operations.
#[tokio::test]
#[ignore = "flaky concurrency test - timing dependent on system load"]
async fn test_concurrent_pipeline_processing() {
    struct ConcurrentTestProcessor;

    impl Plugin for ConcurrentTestProcessor {
        fn name(&self) -> &str {
            "concurrent-test"
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
    impl PostProcessor for ConcurrentTestProcessor {
        async fn process(&self, result: &mut ExtractedDocument, _: &ExtractionConfig) -> Result<()> {
            tokio::time::sleep(Duration::from_millis(10)).await;
            result.content.push_str("[processed]");
            Ok(())
        }

        fn processing_stage(&self) -> ProcessingStage {
            ProcessingStage::Early
        }
    }

    let registry = get_post_processor_registry();
    {
        let mut reg = registry.write();
        let processor = Arc::new(ConcurrentTestProcessor);
        let _ = reg.remove("concurrent-test");
        reg.register(processor).expect("Should register processor");
    }

    let config = ExtractionConfig {
        postprocessor: Some(PostProcessorConfig {
            enabled: true,
            enabled_processors: Some(vec!["concurrent-test".to_string()]),
            disabled_processors: None,
            enabled_set: None,
            disabled_set: None,
        }),
        ..Default::default()
    };

    let mut handles = vec![];
    for i in 0..50 {
        let config = config.clone();

        handles.push(tokio::spawn(async move {
            let mut doc = InternalDocument::new("text");
            doc.mime_type = "text/plain".to_string();
            doc.elements.push(InternalElement::text(
                ElementKind::Paragraph,
                format!("Content {}", i),
                0,
            ));

            run_pipeline(doc, &config).await
        }));
    }

    for handle in handles {
        let result = handle.await.expect("Task should not panic");
        assert!(result.is_ok(), "Pipeline should succeed");
        let processed = result.expect("Operation failed");
        assert!(processed.content.contains("[processed]"), "Processor should run");
    }

    {
        let mut reg = registry.write();
        let _ = reg.remove("concurrent-test");
    }
}

/// Test concurrent registry reads don't block unnecessarily.
///
/// Validates that:
/// - Multiple readers can access registry simultaneously
/// - Registry lookups are fast under concurrent load
#[tokio::test]
async fn test_concurrent_registry_reads() {
    let registry = get_document_extractor_registry();

    let mut handles = vec![];
    for _ in 0..200 {
        let registry_clone = Arc::clone(&registry);
        handles.push(tokio::spawn(async move {
            let start = std::time::Instant::now();

            let reg = registry_clone.read();
            let _extractor = reg.get("text/plain");

            start.elapsed()
        }));
    }

    let mut max_duration = Duration::from_secs(0);
    for handle in handles {
        let duration = handle.await.expect("Task should not panic");
        if duration > max_duration {
            max_duration = duration;
        }
    }

    assert!(
        max_duration < Duration::from_millis(10),
        "Registry reads should be fast, max duration: {:?}",
        max_duration
    );
}

/// Test that extraction throughput scales with concurrency.
///
/// Validates that:
/// - Parallel extractions are actually running in parallel
/// - No global bottlenecks limiting throughput
///
/// Note: This is a performance benchmark that can be flaky based on system load,
/// CPU availability, and other factors. Marked as #[ignore] to run only on demand.
#[tokio::test]
#[ignore]
async fn test_extraction_throughput_scales() {
    let config = ExtractionConfig::default();
    let test_data = b"Throughput test content";

    let sequential_start = std::time::Instant::now();
    for _ in 0..20 {
        let _ = extract_bytes_document(test_data, "text/plain", &config)
            .await
            .expect("Async operation failed");
    }
    let sequential_duration = sequential_start.elapsed();

    let parallel_start = std::time::Instant::now();
    let mut handles = vec![];
    for _ in 0..20 {
        let config = config.clone();
        let data = test_data.to_vec();

        handles.push(tokio::spawn(async move {
            extract_bytes_document(&data, "text/plain", &config).await
        }));
    }

    for handle in handles {
        let _ = handle.await.expect("Task should not panic");
    }
    let parallel_duration = parallel_start.elapsed();

    println!(
        "Sequential: {:?}, Parallel: {:?}, Speedup: {:.2}x",
        sequential_duration,
        parallel_duration,
        sequential_duration.as_secs_f64() / parallel_duration.as_secs_f64()
    );

    let speedup = sequential_duration.as_secs_f64() / parallel_duration.as_secs_f64();

    assert!(
        speedup > 0.5,
        "Parallel execution should not be significantly slower than sequential. Sequential: {:?}, Parallel: {:?}, Speedup: {:.2}x",
        sequential_duration,
        parallel_duration,
        speedup
    );
}

/// High-load stress test with many concurrent operations.
///
/// Validates system stability under sustained concurrent load.
#[tokio::test]
async fn test_high_concurrency_stress() {
    let config = ExtractionConfig {
        use_cache: true,
        ..Default::default()
    };

    #[allow(unused_mut)]
    let mut formats = vec![
        (b"Text content" as &[u8], "text/plain"),
        (b"{\"json\": true}", "application/json"),
        (b"# Markdown\n\nContent", "text/markdown"),
    ];

    #[cfg(feature = "xml")]
    formats.push((b"<xml><item>content</item></xml>" as &[u8], "application/xml"));

    let mut handles = vec![];
    for _ in 0..100 {
        for (data, mime_type) in &formats {
            let config = config.clone();
            let data = data.to_vec();
            let mime_type = mime_type.to_string();

            handles.push(tokio::spawn(async move {
                extract_bytes_document(&data, &mime_type, &config).await
            }));
        }
    }

    let results = timeout(Duration::from_secs(60), async {
        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.expect("Task should not panic"));
        }
        results
    })
    .await
    .expect("High-load stress test should complete within 60s");

    let expected_successes = 100 * formats.len();
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        success_count, expected_successes,
        "All extractions should succeed under stress, got {} successes",
        success_count
    );
}
