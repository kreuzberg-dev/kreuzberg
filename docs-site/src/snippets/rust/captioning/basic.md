```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, CaptioningConfig, LlmConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        captioning: Some(CaptioningConfig {
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
            prompt: None,
            min_image_area: 1000,
        }),
        ..Default::default()
    };
    let output = extract(ExtractInput::from_uri("report.pdf"), &config).await?;
    for image in output.results[0].images.iter().flatten() {
        if let Some(caption) = &image.caption {
            println!("{caption}");
        }
    }
    Ok(())
}
```
