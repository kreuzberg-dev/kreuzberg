---
id: fixture_rust_config_llm_structured_extraction
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"structured_extraction":{"llm":{"model":"openai/gpt-4o"},"schema":{"properties":{"date":{"type":"string"},"summary":{"type":"string"},"title":{"type":"string"}},"required":["title"],"type":"object"},"schema_name":"memo_data"}}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].structured_data);
}

```
