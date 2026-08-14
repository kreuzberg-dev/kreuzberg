```rust title="Rust"
use xberg::plugins::{Plugin, PostProcessor, ProcessingStage};
use xberg::{Result, ExtractedDocument, ExtractionConfig, ProcessingWarning};
use async_trait::async_trait;

struct WordCountProcessor;

impl Plugin for WordCountProcessor {
    fn name(&self) -> &str { "word-count" }
    fn version(&self) -> String { "1.0.0".to_string() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl PostProcessor for WordCountProcessor {
    async fn process(
        &self,
        result: &mut ExtractedDocument,
        _config: &ExtractionConfig
    ) -> Result<()> {
        let word_count = result.content.split_whitespace().count();

        result.processing_warnings.push(ProcessingWarning {
            source: "word-count".into(),
            message: format!("Processed with word count: {}", word_count).into()
        });

        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Early
    }

    fn should_process(
        &self,
        result: &ExtractedDocument,
        _config: &ExtractionConfig
    ) -> bool {
        !result.content.is_empty()
    }
}
```
