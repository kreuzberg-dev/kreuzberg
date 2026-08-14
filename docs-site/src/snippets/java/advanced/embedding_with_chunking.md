```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractInput;
import io.xberg.ChunkingConfig;
import io.xberg.Chunk;
import io.xberg.EmbeddingConfig;
import io.xberg.EmbeddingModelType;
import java.util.List;

ExtractionConfig config = ExtractionConfig.builder()
    .withChunking(ChunkingConfig.builder()
        .withMaxCharacters(512L)
        .withOverlap(50L)
        .withEmbedding(EmbeddingConfig.builder()
            .withModel(new EmbeddingModelType.Preset("balanced"))
            .withNormalize(true)
            .withBatchSize(32L)
            .withShowDownloadProgress(false)
            .build())
        .build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
    config
);
ExtractedDocument result = output.results().get(0);
List<Chunk> chunks = result.chunks() != null ? result.chunks() : List.of();
for (int index = 0; index < chunks.size(); index++) {
    Chunk chunk = chunks.get(index);
    String chunkId = "doc_chunk_" + index;
    System.out.println("Chunk " + chunkId + ": " + chunk.content().substring(0, Math.min(50, chunk.content().length())));
    List<Float> embedding = chunk.embedding();
    if (embedding != null) {
        System.out.println("  Embedding dimensions: " + embedding.size());
    }
}
```
