//! Batch processing integration tests.
//!
//! Tests for `extract_uri_documents` and `extract_bytes_documents` functions.
//! Validates concurrent processing, error handling, and performance.

mod helpers;
#[cfg(feature = "pdf")]
use helpers::extract_uri_documents_blocking;
use helpers::{
    BytesInput, UriBatchInput, extract_bytes_documents, extract_bytes_documents_blocking, extract_uri_documents,
};

use xberg::core::config::ExtractionConfig;

use helpers::{get_test_documents_dir, get_test_file_path, skip_if_missing, test_documents_available};

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

/// Test batch extraction with multiple file formats (PDF, DOCX, TXT).
#[tokio::test]
#[cfg(all(feature = "pdf", feature = "office", feature = "tokio-runtime"))]
async fn test_batch_extract_file_multiple_formats() {
    if !test_documents_available() {
        println!("Skipping test: test_documents/ directory not found");
        return;
    }

    if skip_if_missing("pdf/fake_memo.pdf")
        || skip_if_missing("documents/fake.docx")
        || skip_if_missing("text/fake_text.txt")
    {
        return;
    }

    let config = ExtractionConfig::default();

    let paths: Vec<UriBatchInput> = vec![
        get_test_file_path("pdf/fake_memo.pdf"),
        get_test_file_path("documents/fake.docx"),
        get_test_file_path("text/fake_text.txt"),
    ]
    .into_iter()
    .map(|path| UriBatchInput { path, config: None })
    .collect();

    let results = extract_uri_documents(paths, &config).await;

    assert!(results.is_ok(), "Batch extraction should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 3);

    assert!(!results[0].content.is_empty(), "PDF content should not be empty");
    assert_eq!(results[0].mime_type, "application/pdf");

    assert!(!results[1].content.is_empty(), "DOCX content should not be empty");
    assert_eq!(
        results[1].mime_type,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );

    assert!(!results[2].content.is_empty(), "TXT content should not be empty");
    assert_eq!(results[2].mime_type, "text/plain");

    assert!(results[0].metadata.error.is_none());
    assert!(results[1].metadata.error.is_none());
    assert!(results[2].metadata.error.is_none());
}

/// Test synchronous batch extraction variant.
#[test]
#[cfg(feature = "pdf")]
fn test_batch_extract_file_sync_variant() {
    if !test_documents_available() {
        println!("Skipping test: test_documents/ directory not found");
        return;
    }

    if skip_if_missing("pdf/fake_memo.pdf") || skip_if_missing("text/fake_text.txt") {
        return;
    }

    let config = ExtractionConfig::default();

    let paths: Vec<UriBatchInput> = vec![
        get_test_file_path("pdf/fake_memo.pdf"),
        get_test_file_path("text/fake_text.txt"),
    ]
    .into_iter()
    .map(|path| UriBatchInput { path, config: None })
    .collect();

    let results = extract_uri_documents_blocking(paths, &config);

    assert!(results.is_ok(), "Sync batch extraction should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 2);

    assert!(!results[0].content.is_empty(), "PDF content should not be empty");
    assert_eq!(
        results[0].mime_type, "application/pdf",
        "PDF MIME type should be correct"
    );
    assert!(results[0].metadata.error.is_none(), "PDF should extract without errors");

    assert!(!results[1].content.is_empty(), "Text content should not be empty");
    assert_eq!(results[1].mime_type, "text/plain", "Text MIME type should be correct");
    assert!(
        results[1].metadata.error.is_none(),
        "Text should extract without errors"
    );
}

/// Test batch extraction from bytes.
#[tokio::test]
async fn test_batch_extract_bytes_multiple() {
    let config = ExtractionConfig::default();

    let text_bytes = b"This is plain text content";
    let markdown_bytes = b"# Markdown Header\n\nThis is markdown content";
    let json_bytes = b"{\"key\": \"value\", \"number\": 42}";

    let contents = vec![
        (text_bytes.as_slice(), "text/plain"),
        (markdown_bytes.as_slice(), "text/markdown"),
        (json_bytes.as_slice(), "application/json"),
    ];

    let owned_contents: Vec<BytesInput> = contents
        .into_iter()
        .map(|(bytes, mime)| BytesInput {
            content: bytes.to_vec(),
            mime_type: mime.to_string(),
            config: None,
        })
        .collect();

    let results = extract_bytes_documents(owned_contents, &config).await;

    assert!(results.is_ok(), "Batch bytes extraction should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 3);

    assert_text_content(&results[0].content, "This is plain text content");
    assert_eq!(results[0].mime_type, "text/plain");

    assert!(results[1].content.contains("Markdown Header"));
    assert_eq!(results[1].mime_type, "text/markdown");

    assert!(results[2].content.contains("key"));
    assert!(results[2].content.contains("value"));
    assert_eq!(results[2].mime_type, "application/json");
}

/// Test batch extraction with empty file list.
#[tokio::test]
async fn test_batch_extract_empty_list() {
    let config = ExtractionConfig::default();

    let paths: Vec<UriBatchInput> = vec![];
    let results = extract_uri_documents(paths, &config).await;

    assert!(results.is_ok(), "Empty batch should succeed");
    assert_eq!(
        results.expect("Operation failed").len(),
        0,
        "Should return empty vector"
    );
}

/// Test batch extraction when one file fails (others should succeed).
#[tokio::test]
async fn test_batch_extract_one_file_fails() {
    if !test_documents_available() {
        println!("Skipping test: test_documents/ directory not found");
        return;
    }

    if skip_if_missing("text/fake_text.txt") {
        return;
    }

    let config = ExtractionConfig::default();

    let paths: Vec<UriBatchInput> = vec![
        get_test_file_path("text/fake_text.txt"),
        get_test_documents_dir().join("nonexistent_file.txt"),
        get_test_file_path("text/contract.txt"),
    ]
    .into_iter()
    .map(|path| UriBatchInput { path, config: None })
    .collect();

    let results = extract_uri_documents(paths, &config).await;

    assert!(results.is_ok(), "Batch should succeed even with one failure");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 3);

    assert!(!results[0].content.is_empty());
    assert!(results[0].metadata.error.is_none());

    assert!(results[1].metadata.error.is_some());
    assert!(results[1].content.contains("Error:"));

    assert!(!results[2].content.is_empty());
    assert!(results[2].metadata.error.is_none());
}

/// Test batch extraction when all files fail.
#[tokio::test]
async fn test_batch_extract_all_fail() {
    let config = ExtractionConfig::default();

    let test_dir = get_test_documents_dir();
    let paths: Vec<UriBatchInput> = vec![
        test_dir.join("nonexistent1.txt"),
        test_dir.join("nonexistent2.pdf"),
        test_dir.join("nonexistent3.docx"),
    ]
    .into_iter()
    .map(|path| UriBatchInput { path, config: None })
    .collect();

    let results = extract_uri_documents(paths, &config).await;

    assert!(results.is_ok(), "Batch should succeed (errors in metadata)");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 3);

    assert!(results[0].metadata.error.is_some());
    assert!(results[1].metadata.error.is_some());
    assert!(results[2].metadata.error.is_some());

    assert!(results[0].content.contains("Error:"));
    assert!(results[1].content.contains("Error:"));
    assert!(results[2].content.contains("Error:"));
}

/// Test concurrent batch processing (verify parallelism).
#[tokio::test]
async fn test_batch_extract_concurrent() {
    if !test_documents_available() {
        println!("Skipping test: test_documents/ directory not found");
        return;
    }

    if skip_if_missing("text/fake_text.txt") {
        return;
    }

    let config = ExtractionConfig::default();

    let base_path = get_test_file_path("text/fake_text.txt");
    let paths: Vec<UriBatchInput> = (0..20)
        .map(|_| UriBatchInput {
            path: base_path.clone(),
            config: None,
        })
        .collect();

    let start = std::time::Instant::now();
    let results = extract_uri_documents(paths, &config).await;
    let duration = start.elapsed();

    assert!(results.is_ok(), "Concurrent batch should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 20);

    for result in &results {
        assert!(result.metadata.error.is_none(), "Result should not have errors");
        assert!(!result.content.is_empty(), "Result content should not be empty");
        assert_eq!(result.mime_type, "text/plain", "MIME type should be text/plain");
    }

    assert!(
        !results[0].content.is_empty(),
        "Should have extracted actual text content"
    );

    assert!(duration.as_secs() < 5, "Batch processing took too long: {:?}", duration);
}

/// Test large batch (50+ files).
#[tokio::test]
async fn test_batch_extract_large_batch() {
    if !test_documents_available() {
        println!("Skipping test: test_documents/ directory not found");
        return;
    }

    if skip_if_missing("text/fake_text.txt") {
        return;
    }

    let config = ExtractionConfig::default();

    let base_path = get_test_file_path("text/fake_text.txt");
    let paths: Vec<UriBatchInput> = (0..50)
        .map(|_| UriBatchInput {
            path: base_path.clone(),
            config: None,
        })
        .collect();

    let results = extract_uri_documents(paths, &config).await;

    assert!(results.is_ok(), "Large batch should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 50);

    for result in &results {
        assert!(result.metadata.error.is_none());
        assert!(!result.content.is_empty());
        assert_eq!(result.mime_type, "text/plain");
    }
}

/// Test sync variant with bytes.
#[test]
fn test_batch_extract_bytes_sync_variant() {
    let config = ExtractionConfig::default();

    let contents = vec![
        (b"content 1".as_slice(), "text/plain"),
        (b"content 2".as_slice(), "text/plain"),
        (b"# content 3".as_slice(), "text/markdown"),
    ];

    let owned_contents: Vec<BytesInput> = contents
        .into_iter()
        .map(|(bytes, mime)| BytesInput {
            content: bytes.to_vec(),
            mime_type: mime.to_string(),
            config: None,
        })
        .collect();

    let results = extract_bytes_documents_blocking(owned_contents, &config);

    assert!(results.is_ok(), "Sync batch bytes extraction should succeed");
    let results = results.expect("Operation failed");

    assert_eq!(results.len(), 3);
    assert_text_content(&results[0].content, "content 1");
    assert_text_content(&results[1].content, "content 2");
    assert!(results[2].content.contains("content 3"));
}
