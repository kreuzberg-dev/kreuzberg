```rust title="Rust"
use xberg::{ExtractionConfig, TokenReductionOptions};

let config = ExtractionConfig {
    token_reduction: Some(TokenReductionOptions {
        mode: "moderate".to_string(),
        preserve_important_words: true,
    }),
    ..Default::default()
};
```
