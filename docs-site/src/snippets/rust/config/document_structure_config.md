```rust title="Document Structure Config (Rust)"
use xberg::{extract, ExtractionConfig, ExtractInput};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        include_document_structure: true,
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    let result = &output.results[0];

    if let Some(document) = &result.document {
        for node in &document.nodes {
            let text = node.content.text().unwrap_or("");
            println!("[{}] {}", node.content.node_type_str(), &text[..text.len().min(80)]);
        }
    }
    Ok(())
}
```
