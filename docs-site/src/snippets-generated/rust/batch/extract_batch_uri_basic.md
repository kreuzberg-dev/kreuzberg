---
id: fixture_rust_extract_batch_uri_basic
language: rust
target: rust
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```rust title="Rust"
use xberg::extract_batch;
use xberg::ExtractInput;

#[tokio::main]
async fn main() {
    let inputs_json: serde_json::Value = serde_json::from_str(r#"[{"kind":"uri","uri":"pdf/fake_memo.pdf"},{"kind":"uri","uri":"text/fake_text.txt"}]"#).unwrap();
    let inputs = serde_json::from_value::<Vec<ExtractInput>>(inputs_json).unwrap();
    let config = Default::default();
    let result = extract_batch(inputs, &config).await.expect("call failed");
    for result in result.results {
        println!("{}", result.content);
    }
}

```
