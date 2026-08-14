```rust title="Rust"
use xberg::{ExtractionConfig, ChunkingConfig};

let config = ExtractionConfig {
    chunking: Some(ChunkingConfig {
        max_characters: 1000,
        overlap: 200,
        embedding: None,
        ..Default::default()
    }),
    ..Default::default()
};
```

```rust title="Rust - Semantic"
use xberg::{ExtractionConfig, ChunkingConfig, ChunkerType};

let config = ExtractionConfig {
    chunking: Some(ChunkingConfig {
        chunker_type: ChunkerType::Semantic,
        ..Default::default()
    }),
    ..Default::default()
};
```

```rust title="Rust - Prepend Heading Context"
use xberg::{ExtractionConfig, ChunkingConfig, ChunkerType};

let config = ExtractionConfig {
    chunking: Some(ChunkingConfig {
        max_characters: 500,
        overlap: 50,
        chunker_type: ChunkerType::Markdown,
        prepend_heading_context: true,
        ..Default::default()
    }),
    ..Default::default()
};
```
