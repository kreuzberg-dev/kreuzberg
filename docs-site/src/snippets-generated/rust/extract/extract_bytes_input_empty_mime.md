---
id: fixture_rust_extract_bytes_input_empty_mime
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with empty MIME type

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"bytes":"test_documents/text/plain.txt","config":{},"filename":"plain.txt","kind":"bytes","mime_type":""}"#).unwrap();
    let input_file_0 = std::fs::read(r#"test_documents/text/plain.txt"#).expect("file read failed");
    *input_json.pointer_mut(r#"/bytes"#).expect("docs file field missing") = serde_json::json!(input_file_0);
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let _ = extract(input, &config).await;
}

```
