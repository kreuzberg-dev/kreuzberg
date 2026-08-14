---
id: fixture_rust_api_extract_batch_bytes
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction API (extract_batch)

```rust title="Rust"
use xberg::extract_batch;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let inputs_json: serde_json::Value = serde_json::from_str(r#"[{"bytes":"test_documents/pdf/fake_memo.pdf","filename":"fake_memo.pdf","kind":"bytes"}]"#).unwrap();
    let inputs_file_0 = std::fs::read(r#"test_documents/pdf/fake_memo.pdf"#).expect("file read failed");
    *inputs_json.pointer_mut(r#"/0/bytes"#).expect("docs file field missing") = serde_json::json!(inputs_file_0);
    let inputs = serde_json::from_value::<Vec<ExtractInput>>(inputs_json).unwrap();
    let config = Default::default();
    let _ = extract_batch(inputs, &config).await;
}

```
