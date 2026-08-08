#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression tests for #155 (structural rendering for YAML/TOML/JSONL and
//! top-level JSON arrays) and #166 (surfacing the flattened structured-data
//! view instead of discarding it).

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// #155: a top-level JSON array of objects must render as headings/list items,
/// not as one opaque code block.
#[tokio::test]
async fn should_render_structure_for_top_level_json_array() {
    let config = ExtractionConfig::default();
    let json = br#"[{"name": "Ada"}, {"name": "Grace"}]"#;

    let extraction = extract_bytes_document(json, "application/json", &config)
        .await
        .expect("json extraction should succeed");

    assert!(
        extraction.content.contains("Item 1") && extraction.content.contains("Item 2"),
        "expected per-item headings, got: {}",
        extraction.content
    );
    assert!(
        extraction.content.contains("name: Ada") && extraction.content.contains("name: Grace"),
        "expected rendered field values, got: {}",
        extraction.content
    );
    assert!(
        !extraction.content.trim_start().starts_with('['),
        "content should not be a raw JSON code block: {}",
        extraction.content
    );
}

/// #155: YAML must render structure (headings), not fall through to a raw code block.
#[tokio::test]
async fn should_render_structure_for_yaml() {
    let config = ExtractionConfig::default();
    let yaml = b"name: Alice\nrole: Engineer\n";

    let extraction = extract_bytes_document(yaml, "application/yaml", &config)
        .await
        .expect("yaml extraction should succeed");

    assert!(
        extraction.content.contains("name: Alice"),
        "expected rendered field, got: {}",
        extraction.content
    );
    assert!(
        !extraction.content.contains("name: Alice\nrole: Engineer"),
        "content should not be the verbatim raw YAML code block: {}",
        extraction.content
    );
}

/// #155: TOML must render structure (headings) for a table, not fall through to a raw
/// code block.
#[tokio::test]
async fn should_render_structure_for_toml() {
    let config = ExtractionConfig::default();
    let toml = b"[user]\nname = \"Alice\"\nrole = \"Engineer\"\n";

    let extraction = extract_bytes_document(toml, "application/toml", &config)
        .await
        .expect("toml extraction should succeed");

    assert!(
        extraction.content.contains("user"),
        "expected a heading for the [user] table, got: {}",
        extraction.content
    );
    assert!(
        extraction.content.contains("name: Alice"),
        "expected rendered field, got: {}",
        extraction.content
    );
}

/// #155: JSONL (an array of records) must render per-record structure instead of one
/// opaque code block.
#[tokio::test]
async fn should_render_structure_for_jsonl() {
    let config = ExtractionConfig::default();
    let jsonl = b"{\"name\": \"Alice\"}\n{\"name\": \"Bob\"}";

    let extraction = extract_bytes_document(jsonl, "application/x-ndjson", &config)
        .await
        .expect("jsonl extraction should succeed");

    assert!(
        extraction.content.contains("Item 1") && extraction.content.contains("Item 2"),
        "expected per-record headings, got: {}",
        extraction.content
    );
    assert!(extraction.content.contains("name: Alice"));
    assert!(extraction.content.contains("name: Bob"));
}

/// #166: the flattened `path: value` view must be surfaced in metadata instead of
/// silently discarded.
#[tokio::test]
async fn should_surface_flattened_fields_in_metadata() {
    let config = ExtractionConfig::default();
    let json = br#"{"user": {"name": "Alice", "role": "Engineer"}}"#;

    let extraction = extract_bytes_document(json, "application/json", &config)
        .await
        .expect("json extraction should succeed");

    let flattened = extraction
        .metadata
        .additional
        .get("flattened_fields")
        .expect("flattened_fields metadata must be present")
        .as_array()
        .expect("flattened_fields must be a JSON array");

    let flattened: Vec<&str> = flattened.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(
        flattened,
        vec!["user.name: Alice", "user.role: Engineer"],
        "flattened view should contain every leaf path:value pair"
    );
}
