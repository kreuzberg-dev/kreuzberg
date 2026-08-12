---
id: fixture_rust_format_pdf_text
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"filename":"fake_memo.pdf","kind":"uri","mime_type":"application/pdf","uri":"https://example.com/pdf/fake_memo.pdf"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config = Default::default();
    let result = extract(input, &config).await.expect("call failed");
    println!("{:?}", result.results[0].metadata);
}

```
