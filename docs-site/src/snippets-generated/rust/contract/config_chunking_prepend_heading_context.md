---
id: fixture_rust_config_chunking_prepend_heading_context
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"document.md"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"chunking":{"chunker_type":"markdown","max_characters":500,"overlap":50,"prepend_heading_context":true}}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    for chunk in result.results[0].chunks.iter().flatten() {
        println!("{}", chunk.content);
        println!("{}", chunk.metadata);
    }
}

```
