---
id: fixture_rust_output_format_bytes_markdown
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"bytes":"test_documents/pdf/fake_memo.pdf","config":{"output_format":"markdown"},"filename":"fake_memo.pdf","kind":"bytes","mime_type":"application/pdf"}"#).unwrap();
    let input_file_0 = std::fs::read(r#"test_documents/pdf/fake_memo.pdf"#).expect("file read failed");
    *input_json.pointer_mut(r#"/bytes"#).expect("docs file field missing") = serde_json::json!(input_file_0);
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"output_format":"markdown"}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let _ = extract(input, &config).await;
}

```
