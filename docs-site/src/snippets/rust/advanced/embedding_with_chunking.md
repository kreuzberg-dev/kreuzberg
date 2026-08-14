```rust title="Rust"
use xberg::{ExtractionConfig, ChunkingConfig, EmbeddingConfig, EmbeddingModelType};

let config = ExtractionConfig {
    chunking: Some(ChunkingConfig {
        max_characters: 1024,
        overlap: 100,
        embedding: Some(EmbeddingConfig {
            model: EmbeddingModelType::Preset { name: "balanced".to_string() },
            normalize: true,
            batch_size: 32,
            show_download_progress: false,
            ..Default::default()
        }),
        ..Default::default()
    }),
    ..Default::default()
};
```
