```rust title="Rust"
use xberg::{ExtractionConfig, api::serve_with_config};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig::discover()?.unwrap_or_default();
    serve_with_config("0.0.0.0", 8000, config).await?;
    Ok(())
}

```
