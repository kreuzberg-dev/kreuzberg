---
id: fixture_rust_format_docx_equations
language: rust
target: rust
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```rust title="Rust"
use xberg::extract;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let input_json: serde_json::Value = serde_json::from_str(r#"{"filename":"equations.docx","kind":"uri","mime_type":"application/vnd.openxmlformats-officedocument.wordprocessingml.document","uri":"https://example.com/docx/equations.docx"}"#).unwrap();
    let input = serde_json::from_value::<ExtractInput>(input_json).unwrap();
    let config_json: serde_json::Value = serde_json::from_str(r#"{"output_format":"markdown"}"#).unwrap();
    let config = serde_json::from_value(config_json).unwrap();
    let _ = extract(input, &config).await;
}

```
