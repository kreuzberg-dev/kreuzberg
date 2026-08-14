```rust title="Rust"
use xberg::plugins::{Plugin, PostProcessor, ProcessingStage};
use xberg::{ExtractedDocument, ExtractionConfig, Result};
use async_trait::async_trait;

struct PdfOnlyProcessor;

impl Plugin for PdfOnlyProcessor {
    fn name(&self) -> &str {
        "pdf-only"
    }
    fn version(&self) -> String {
        "1.0.0".to_string()
    }
    fn initialize(&self) -> Result<()> {
        Ok(())
    }
    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl PostProcessor for PdfOnlyProcessor {
    async fn process(
        &self,
        result: &mut ExtractedDocument,
        _config: &ExtractionConfig
    ) -> Result<()> {
        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }

    fn should_process(
        &self,
        result: &ExtractedDocument,
        _config: &ExtractionConfig
    ) -> bool {
        result.mime_type == "application/pdf"
    }
}
```
