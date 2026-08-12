---
id: fixture_rust_config_keywords
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"keywords":{"algorithm":"yake","max_keywords":10}}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    for keyword in result.results[0].keywords {
        println!("{}", keyword.text);
        println!("{}", keyword.score);
    }
}

```
