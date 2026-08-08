//! Regression test for #165 — JSON/YAML/TOML keys must render in document order.
//!
//! The audit recorded this as a live defect on the grounds that `serde_json` was declared
//! without `preserve_order` and the feature was absent from `Cargo.lock`. That turned out to
//! be wrong: `Cargo.lock`'s `serde_json` entry depends on `indexmap`, which only happens when
//! `preserve_order` is on — `serde_toon_format` was requesting it transitively, so key order
//! was already preserved and no behaviour needed to change.
//!
//! What was genuinely wrong is that the guarantee rested on an unrelated dependency's feature
//! selection. `crates/xberg/Cargo.toml` now requests `preserve_order` explicitly, and this
//! test fails if that is ever lost: without it `Value::Object` is a `BTreeMap` and the keys
//! below come back alphabetised.

use xberg::core::config::ExtractionConfig;

mod helpers;
use helpers::extract_bytes_document;

/// Deliberately anti-alphabetical: sorted order would be `alpha, middle, zebra`.
const DOCUMENT_ORDER: [&str; 3] = ["zebra", "alpha", "middle"];

fn assert_document_order(content: &str, label: &str) {
    let positions: Vec<usize> = DOCUMENT_ORDER
        .iter()
        .map(|key| {
            content
                .find(key)
                .unwrap_or_else(|| panic!("{label}: key {key:?} missing from output {content:?}"))
        })
        .collect();

    assert!(
        positions[0] < positions[1] && positions[1] < positions[2],
        "{label}: keys must render in document order (zebra, alpha, middle), not alphabetised; \
         got offsets {positions:?} in {content:?}"
    );
}

#[tokio::test]
async fn should_preserve_json_key_order() {
    let json = br#"{"zebra": 1, "alpha": 2, "middle": 3}"#;
    let result = extract_bytes_document(json, "application/json", &ExtractionConfig::default())
        .await
        .expect("JSON extraction should succeed");
    assert_document_order(&result.content, "json");
}

#[tokio::test]
async fn should_preserve_yaml_key_order() {
    let yaml = b"zebra: 1\nalpha: 2\nmiddle: 3\n";
    let result = extract_bytes_document(yaml, "application/yaml", &ExtractionConfig::default())
        .await
        .expect("YAML extraction should succeed");
    assert_document_order(&result.content, "yaml");
}
