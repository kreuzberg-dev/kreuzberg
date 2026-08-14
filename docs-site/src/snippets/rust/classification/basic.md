```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, PageClassificationConfig, LlmConfig};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        page_classification: Some(PageClassificationConfig {
            labels: vec!["invoice".into(), "contract".into(), "id_document".into(), "receipt".into()],
            multi_label: false,
            prompt_template: None,
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
        }),
        ..Default::default()
    };
    let _output = extract(ExtractInput::from_uri("packet.pdf"), &config).await?;
    Ok(())
}
```
