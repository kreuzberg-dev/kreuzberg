```rust title="Rust"
use xberg::{
    extract, ExtractionConfig, ExtractInput, LlmConfig, StructuredExtractionConfig,
};
use serde_json::json;

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        structured_extraction: Some(StructuredExtractionConfig {
            schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "authors": { "type": "array", "items": { "type": "string" } },
                    "date": { "type": "string" }
                },
                "required": ["title", "authors", "date"],
                "additionalProperties": false
            }),
            schema_name: "extraction".to_string(),
            schema_description: None,
            prompt: None,
            strict: true,
            llm: LlmConfig {
                model: "openai/gpt-4o-mini".to_string(),
                ..Default::default()
            },
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("paper.pdf"), &config).await?;
    if let Some(structured) = &output.results[0].structured_output {
        println!("{}", structured);
    }
    Ok(())
}
```
