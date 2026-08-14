---
id: fixture_rust_ocr_image_png
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let mut input_json: serde_json::Value = serde_json::from_str(r#"{"bytes":"test_documents/images/test_hello_world.png","config":{},"filename":"test_hello_world.png","kind":"bytes","mime_type":"image/png"}"#).unwrap();
    let input_file_0 = std::fs::read(r#"test_documents/images/test_hello_world.png"#).expect("file read failed");
    *input_json.pointer_mut(r#"/bytes"#).expect("docs file field missing") = serde_json::json!(input_file_0);
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].content);
}

```
