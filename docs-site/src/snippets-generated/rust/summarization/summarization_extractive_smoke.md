---
id: fixture_rust_summarization_extractive_smoke
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","uri":"https://example.com/text/book_war_and_peace_1p.txt"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"summarization":{"max_tokens":80,"strategy":"extractive"}}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].summary);
}

```
