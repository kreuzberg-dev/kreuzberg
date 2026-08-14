```rust title="Rust"
use xberg::plugins::{Plugin, OcrBackend, OcrBackendType};
use xberg::{Result, ExtractedDocument, OcrConfig};
use async_trait::async_trait;

struct CloudOcrBackend {
    api_key: String,
    supported_langs: Vec<String>,
}

impl Plugin for CloudOcrBackend {
    fn name(&self) -> &str { "cloud-ocr" }
    fn version(&self) -> String { "1.0.0".to_string() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl OcrBackend for CloudOcrBackend {
    async fn process_image(
        &self,
        image_bytes: &[u8],
        config: &OcrConfig,
    ) -> Result<ExtractedDocument> {
        let language = config.language.first().map(String::as_str).unwrap_or("eng");
        let text = self.call_cloud_api(image_bytes, language).await?;

        // `ExtractedDocument` has private internal fields, so a struct literal with
        // `..Default::default()` does not compile outside the crate. Build a default
        // and assign the public fields instead.
        let mut document = ExtractedDocument::default();
        document.content = text;
        document.mime_type = "text/plain".into();
        Ok(document)
    }

    fn supports_language(&self, lang: &str) -> bool {
        self.supported_langs.iter().any(|l| l == lang)
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Custom
    }

    fn supported_languages(&self) -> Vec<String> {
        self.supported_langs.clone()
    }
}

impl CloudOcrBackend {
    async fn call_cloud_api(
        &self,
        image: &[u8],
        language: &str
    ) -> Result<String> {
        Ok("Extracted text".to_string())
    }
}
```
