---
id: fixture_rust_extract_batch_bytes_size_cap
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```rust title="Rust"
use xberg::extract_batch;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let inputs_json: serde_json::Value = serde_json::from_str(r#"[{"bytes":"test_documents/text/fake_text.txt","kind":"bytes","mime_type":"text/plain"}]"#).unwrap();
    let inputs_file_0 = std::fs::read(r#"test_documents/text/fake_text.txt"#).expect("file read failed");
    *inputs_json.pointer_mut(r#"/0/bytes"#).expect("docs file field missing") = serde_json::json!(inputs_file_0);
    let inputs = serde_json::from_value::<Vec<ExtractInput>>(inputs_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"security_limits":{"max_content_size":1}}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let _ = extract_batch(inputs, &config).await;
}

```
