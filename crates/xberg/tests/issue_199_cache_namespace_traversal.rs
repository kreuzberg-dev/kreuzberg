//! Regression test for #199 — an unvalidated `cache_namespace` reached
//! `create_dir_all`.
//!
//! `ExtractionConfig::cache_namespace` is caller-controlled (REST API, CLI, and
//! every language binding expose it) and was used verbatim as a directory
//! component under the cache root. A namespace containing `..` therefore made
//! the library create — and, through the cleanup pass, later delete —
//! directories outside its own cache.
//!
//! This test drives the public extraction API with a traversing namespace and
//! asserts that nothing appears outside the cache root, and that extraction
//! still returns the correct content.

use std::fs;
use tempfile::tempdir;
use xberg::core::config::{ExtractInput, ExtractionConfig};

/// `std::env::set_var` is `unsafe` in edition 2024; this binary contains a
/// single test, so no other thread is reading the environment concurrently.
#[allow(unsafe_code)]
fn set_env(key: &str, value: &str) {
    unsafe {
        std::env::set_var(key, value);
    }
}

const DOCUMENT_TEXT: &str = "issue 199 traversal payload";

#[tokio::test]
async fn a_traversing_cache_namespace_must_not_create_directories_outside_the_cache_root() {
    let temp = tempdir().expect("temp dir");

    // The extraction cache lives at <cache root>/extraction, so `../../escaped`
    // resolves to <temp>/escaped — inside the temp dir, but outside the cache.
    let cache_root = temp.path().join("cache-root");
    set_env("XBERG_CACHE_DIR", cache_root.to_str().expect("utf-8 cache root"));

    let source = temp.path().join("document.txt");
    fs::write(&source, DOCUMENT_TEXT).expect("write source document");

    let escaped = temp.path().join("escaped");
    assert!(!escaped.exists(), "precondition: the escape target must not exist yet");

    let config = ExtractionConfig {
        use_cache: true,
        cache_namespace: Some("../../escaped".to_string()),
        ..Default::default()
    };

    let result = xberg::extract(ExtractInput::from_uri(source.to_string_lossy()), &config)
        .await
        .expect("extraction must still succeed when the namespace is rejected");

    assert_eq!(result.results.len(), 1, "exactly one document must be extracted");
    assert_eq!(
        result.results[0].content.trim(),
        DOCUMENT_TEXT,
        "rejecting the namespace must not change the extracted content"
    );

    assert!(
        !escaped.exists(),
        "the cache namespace escaped the cache root and created {}",
        escaped.display()
    );

    // Everything the cache created must sit under the cache root.
    let stray: Vec<_> = fs::read_dir(temp.path())
        .expect("read temp dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name != "cache-root" && name != "document.txt")
        .collect();
    assert!(
        stray.is_empty(),
        "the cache created unexpected entries outside its root: {stray:?}"
    );
}
