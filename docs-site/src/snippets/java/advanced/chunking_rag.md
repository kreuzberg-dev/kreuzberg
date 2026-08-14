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
        .withMaxCharacters(500L)
        .withOverlap(50L)
        .withEmbedding(EmbeddingConfig.builder()
            .withModel(new EmbeddingModelType.Preset("all-mpnet-base-v2"))
            .withNormalize(true)
            .withBatchSize(16L)
            .build())
        .build())
    .build();
try {
    ExtractionResult output = Xberg.extract(
        ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("research_paper.pdf").build(),
        config
    );
    ExtractedDocument result = output.results().get(0);
    List<Chunk> chunks = result.chunks() != null ? result.chunks() : List.of();
    System.out.println("Found " + chunks.size() + " chunks for RAG pipeline");
    for (int i = 0; i < Math.min(3, chunks.size()); i++) {
        Chunk chunk = chunks.get(i);
        System.out.println("Chunk " + i + ": " + chunk.content().substring(0, Math.min(80, chunk.content().length())) + "...");
    }
} catch (Exception ex) {
    System.err.println("RAG extraction failed: " + ex.getMessage());
}
```
