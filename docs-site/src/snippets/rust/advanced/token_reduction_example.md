```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, TokenReductionOptions};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        token_reduction: Some(TokenReductionOptions {
            mode: "moderate".to_string(),
            preserve_important_words: true,
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("verbose_document.pdf"), &config).await?;
    let result = &output.results[0];

    println!("Reduced content length: {} chars", result.content.len());
    Ok(())
}
```
