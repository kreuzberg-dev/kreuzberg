```rust title="Rust"
use std::collections::HashSet;
use xberg::{
    extract, ExtractionConfig, ExtractInput, RedactionConfig, RedactionStrategy,
    types::redaction::PiiCategory,
};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let mut categories = HashSet::new();
    categories.insert(PiiCategory::Email);
    categories.insert(PiiCategory::Phone);
    categories.insert(PiiCategory::Ssn);
    categories.insert(PiiCategory::CreditCard);
    categories.insert(PiiCategory::Iban);

    let config = ExtractionConfig {
        redaction: Some(RedactionConfig {
            categories,
            strategy: RedactionStrategy::Mask,
            ..Default::default()
        }),
        ..Default::default()
    };
    let _output = extract(ExtractInput::from_uri("contract.pdf"), &config).await?;
    Ok(())
}
```
