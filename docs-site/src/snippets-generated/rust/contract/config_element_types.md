---
id: fixture_rust_config_element_types
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/docx/unit_test_headers.docx"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"result_format":"element_based"}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    for element in result.results[0].elements {
        println!("{}", element.element_type);
        println!("{}", element.content);
    }
}

```
