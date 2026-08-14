```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        enable_quality_processing: true,
        ..Default::default()
    };
    let output = extract(ExtractInput::from_uri("scanned_document.pdf"), &config).await?;

    if let Some(score) = output.results[0].quality_score {
        if score < 0.5 {
            println!("Warning: Low quality extraction ({:.2})", score);
        } else {
            println!("Quality score: {:.2}", score);
        }
    }
    Ok(())
}
```
