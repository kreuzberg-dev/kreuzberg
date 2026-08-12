---
id: fixture_rust_api_extract_uri
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config = Default::default();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].content);
}

```
