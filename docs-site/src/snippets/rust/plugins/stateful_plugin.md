```rust title="Rust"
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use xberg::plugins::{Plugin, PostProcessor, ProcessingStage};
use xberg::{ExtractedDocument, ExtractionConfig, Result, XbergError};
use async_trait::async_trait;

struct StatefulPlugin {
    call_count: AtomicUsize,
    cache: Mutex<HashMap<String, String>>,
}

impl Plugin for StatefulPlugin {
    fn name(&self) -> &str { "stateful-plugin" }
    fn version(&self) -> String { "1.0.0".to_string() }

    fn initialize(&self) -> Result<()> {
        self.call_count.store(0, Ordering::Release);
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        let count = self.call_count.load(Ordering::Acquire);
        println!("Plugin called {} times", count);
        Ok(())
    }
}

#[async_trait]
impl PostProcessor for StatefulPlugin {
    async fn process(
        &self,
        result: &mut ExtractedDocument,
        _config: &ExtractionConfig
    ) -> Result<()> {
        self.call_count.fetch_add(1, Ordering::AcqRel);

        let mut cache = self.cache.lock()
            .map_err(|_| XbergError::LockPoisoned("stateful-plugin cache".to_string()))?;
        cache.insert("last_mime".to_string(), result.mime_type.to_string());

        Ok(())
    }

    fn processing_stage(&self) -> ProcessingStage {
        ProcessingStage::Middle
    }
}
```
