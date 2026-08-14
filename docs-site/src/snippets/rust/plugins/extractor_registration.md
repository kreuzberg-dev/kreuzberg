```rust title="Rust"
use xberg::plugins::{Plugin, DocumentExtractor};
use xberg::{ExtractInput, ExtractionConfig, ExtractedDocument, Result, register_document_extractor};
use async_trait::async_trait;
use std::sync::Arc;

struct CustomJsonExtractor;

impl Plugin for CustomJsonExtractor {
    fn name(&self) -> &str { "custom-json-extractor" }
    fn version(&self) -> String { "1.0.0".to_string() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl DocumentExtractor for CustomJsonExtractor {
    async fn extract(&self, input: ExtractInput, _config: &ExtractionConfig) -> Result<ExtractedDocument> {
        let bytes = input.bytes.unwrap_or_default();
        let mut document = ExtractedDocument::default();
        document.content = String::from_utf8_lossy(&bytes).to_string();
        document.mime_type = "application/json".into();
        Ok(document)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/json", "text/json"]
    }
}

fn register_custom_extractor() -> Result<()> {
    let extractor = Arc::new(CustomJsonExtractor);
    register_document_extractor(extractor)?;
    Ok(())
}
```
