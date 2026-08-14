```rust title="Rust"
use xberg::{EmbeddingConfig, EmbeddingModelType, embed_texts};

let config = EmbeddingConfig {
    model: EmbeddingModelType::Preset { name: "balanced".to_string() },
    normalize: true,
    ..Default::default()
};

let texts = vec!["Hello, world!".to_string(), "Xberg is fast".to_string()];
let embeddings = embed_texts(texts, &config)?;

assert_eq!(embeddings.len(), 2);
assert_eq!(embeddings[0].len(), 768);
```
