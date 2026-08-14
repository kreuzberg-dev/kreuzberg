```rust title="Rust"
use xberg::plugins::{Plugin, Validator};
use xberg::{ExtractedDocument, ExtractionConfig, Result, XbergError};
use async_trait::async_trait;

struct QualityValidator;

impl Plugin for QualityValidator {
    fn name(&self) -> &str { "quality-validator" }
    fn version(&self) -> String { "1.0.0".to_string() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl Validator for QualityValidator {
    async fn validate(
        &self,
        result: &ExtractedDocument,
        _config: &ExtractionConfig,
    ) -> Result<()> {
        let score = result.metadata
            .additional
            .get("quality_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        if score < 0.5 {
            return Err(XbergError::validation(format!(
                "Quality score too low: {:.2} < 0.50",
                score
            )));
        }

        Ok(())
    }
}
```
