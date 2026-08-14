```java title="Java"
import io.xberg.ChunkingConfig;
import io.xberg.EmbeddingConfig;
import io.xberg.EmbeddingModelType;
import io.xberg.ExtractionConfig;

ExtractionConfig config = ExtractionConfig.builder()
    .withChunking(ChunkingConfig.builder()
        .withMaxCharacters(1500L)
        .withOverlap(200L)
        .withEmbedding(EmbeddingConfig.builder()
            .withModel(new EmbeddingModelType.Preset("text-embedding-all-minilm-l6-v2"))
            .build())
        .build())
    .build();
```
