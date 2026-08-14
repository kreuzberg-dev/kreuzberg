```rust title="Rust"
use xberg::{ExtractionConfig, mcp::start_mcp_server_with_config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = ExtractionConfig::discover()?.unwrap_or_default();
    start_mcp_server_with_config(config).await?;
    Ok(())
}
```
