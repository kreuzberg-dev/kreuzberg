---
id: fixture_rust_smoke_html_basic
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"kind":"uri","mime_type":"text/html","uri":"https://example.com/html/simple_table.html"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    for table in result.results[0].tables {
        println!("{}", table.rows);
    }
}

```
