```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, ChunkingConfig, EmbeddingConfig, EmbeddingModelType};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let config = ExtractionConfig {
        chunking: Some(ChunkingConfig {
            max_characters: 500,
            overlap: 50,
            embedding: Some(EmbeddingConfig {
                model: EmbeddingModelType::Preset { name: "balanced".to_string() },
                normalize: true,
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let output = extract(ExtractInput::from_uri("research_paper.pdf"), &config).await?;
    let result = &output.results[0];

    if let Some(chunks) = &result.chunks {
        for chunk in chunks {
            println!("Chunk {}/{}",
                chunk.metadata.chunk_index + 1,
                chunk.metadata.total_chunks
            );
            println!("Position: {}-{}",
                chunk.metadata.byte_start,
                chunk.metadata.byte_end
            );
            println!("Content: {}...", &chunk.content[..100.min(chunk.content.len())]);
            if let Some(embedding) = &chunk.embedding {
                println!("Embedding: {} dimensions", embedding.len());
            }
        }
    }
    Ok(())
}
```
