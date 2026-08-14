```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, NerConfig, NerBackendKind, LlmConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        ner: Some(NerConfig {
            backend: NerBackendKind::Llm,
            llm: Some(LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let output = extract(ExtractInput::from_uri("contract.pdf"), &config).await?;
    for entity in output.results[0].entities.iter().flatten() {
        println!("{:?}: {} (confidence={:?})", entity.category, entity.text, entity.confidence);
    }
    Ok(())
}
```
