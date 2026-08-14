---
id: fixture_rust_extract_batch_bytes_happy
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```rust title="Rust"
use xberg::extract_batch;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let inputs_json: serde_json::Value = serde_json::from_str(r#"[{"bytes":[72,101,108,108,111,44,32,119,111,114,108,100,33],"kind":"bytes","mime_type":"text/plain"},{"bytes":"test_documents/html/html.html","kind":"bytes","mime_type":"text/html"}]"#).unwrap();
    let inputs_file_0 = std::fs::read(r#"test_documents/html/html.html"#).expect("file read failed");
    *inputs_json.pointer_mut(r#"/1/bytes"#).expect("docs file field missing") = serde_json::json!(inputs_file_0);
    let inputs = serde_json::from_value::<Vec<ExtractInput>>(inputs_json).unwrap();
    let config = Default::default();
    let _ = extract_batch(inputs, &config).await;
}

```
