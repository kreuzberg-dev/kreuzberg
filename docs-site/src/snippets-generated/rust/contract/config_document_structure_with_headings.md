---
id: fixture_rust_config_document_structure_with_headings
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/docx/fake.docx"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"include_document_structure":true}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].document_structure);
}

```
