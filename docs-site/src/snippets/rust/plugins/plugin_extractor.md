```rust title="Rust"
use xberg::plugins::{DocumentExtractor, Plugin};
use xberg::{Result, ExtractedDocument, ExtractInput, ExtractionConfig};
use async_trait::async_trait;

struct CustomJsonExtractor;

impl Plugin for CustomJsonExtractor {
    fn name(&self) -> &str { "custom-json-extractor" }
    fn version(&self) -> String { "1.0.0".to_string() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl DocumentExtractor for CustomJsonExtractor {
    async fn extract(
        &self,
        input: ExtractInput,
        _config: &ExtractionConfig,
    ) -> Result<ExtractedDocument> {
        let bytes = input.bytes.unwrap_or_default();
        let json: serde_json::Value = serde_json::from_slice(&bytes)?;
        let text = extract_text_from_json(&json);

        let mut document = ExtractedDocument::default();
        document.content = text;
        document.mime_type = "application/json".into();
        Ok(document)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/json", "text/json"]
    }

    fn priority(&self) -> i32 { 50 }
}

fn extract_text_from_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("{}\n", s),
        serde_json::Value::Array(arr) => arr.iter().map(extract_text_from_json).collect(),
        serde_json::Value::Object(obj) => obj.values().map(extract_text_from_json).collect(),
        _ => String::new(),
    }
}
```
