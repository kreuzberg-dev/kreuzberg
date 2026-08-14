---
id: fixture_rust_error_extract_input_conflicting_ocr
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"bytes":"test_documents/text/fake_text.txt","config":{"disable_ocr":true,"force_ocr":true},"filename":"fake_text.txt","kind":"bytes","mime_type":"text/plain"}"#).unwrap();
    let input_file_0 = std::fs::read(r#"test_documents/text/fake_text.txt"#).expect("file read failed");
    *input_json.pointer_mut(r#"/bytes"#).expect("docs file field missing") = serde_json::json!(input_file_0);
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"disable_ocr":true,"force_ocr":true}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let _ = extract(input, &config).await;
}

```
