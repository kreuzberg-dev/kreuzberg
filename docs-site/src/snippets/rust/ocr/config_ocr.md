```rust title="Rust"
use xberg::{ExtractionConfig, OcrConfig, TesseractConfig};

fn main() {
    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string(), "fra".to_string()],
            tesseract_config: Some(TesseractConfig {
                psm: 3,
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
}
```
