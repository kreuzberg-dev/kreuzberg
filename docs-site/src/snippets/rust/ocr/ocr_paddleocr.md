```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, OcrConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "paddleocr".to_string(),
            language: vec!["en".to_string()],
            // paddle_ocr_config: Some(serde_json::json!({"model_tier": "server"})), // for max accuracy
            ..Default::default()
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    println!("Extracted text: {}", output.results[0].content);
    Ok(())
}
```
