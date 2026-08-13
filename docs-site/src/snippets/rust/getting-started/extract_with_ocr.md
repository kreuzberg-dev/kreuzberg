```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, OcrConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        force_ocr: true,
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: "eng".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("scanned.pdf"), &config).await?;
    println!("{}", output.results[0].content);
    println!("Detected languages: {:?}", output.results[0].detected_languages);
    Ok(())
}
```
