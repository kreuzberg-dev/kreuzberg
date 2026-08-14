```java title="Java"
import io.xberg.Xberg;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractInput;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;
import io.xberg.ExtractionConfig;
import io.xberg.ChunkingConfig;
import io.xberg.Chunk;
import io.xberg.EmbeddingConfig;
import io.xberg.EmbeddingModelType;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public class VectorDatabaseIntegration {
    public static class VectorRecord {
        public String id;
        public float[] embedding;
        public String content;
        public Map<String, String> metadata;
    }
    public static List<VectorRecord> extractAndVectorize(String documentPath, String documentId) throws Exception {
        ExtractionConfig config = ExtractionConfig.builder()
            .withChunking(ChunkingConfig.builder()
                .withMaxCharacters(512L)
                .withOverlap(50L)
                .withEmbedding(EmbeddingConfig.builder()
                    .withModel(new EmbeddingModelType.Preset("balanced"))
                    .withNormalize(true)
                    .withBatchSize(32L)
                    .build())
                .build())
            .build();
        ExtractionResult output = Xberg.extract(ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri(documentPath).build(), config);
        ExtractedDocument result = output.results().get(0);
        List<Chunk> chunks = result.chunks() != null ? result.chunks() : List.of();
        List<VectorRecord> vectorRecords = new java.util.ArrayList<>();
        for (int index = 0; index < chunks.size(); index++) {
            Chunk chunk = chunks.get(index);
            VectorRecord record = new VectorRecord();
            record.id = documentId + "_chunk_" + index;
            record.metadata = new HashMap<>();
            record.metadata.put("document_id", documentId);
            record.metadata.put("chunk_index", String.valueOf(index));
            record.content = chunk.content();
            if (chunk.embedding() != null) {
                List<Float> embedding = chunk.embedding();
                record.embedding = new float[embedding.size()];
                for (int i = 0; i < embedding.size(); i++) {
                    record.embedding[i] = embedding.get(i);
                }
            }
            record.metadata.put("content_length", String.valueOf(record.content.length()));
            vectorRecords.add(record);
        }
        storeInVectorDatabase(vectorRecords);
        return vectorRecords;
    }
    
    private static void storeInVectorDatabase(List<VectorRecord> records) {
        for (VectorRecord record : records) {
            if (record.embedding != null && record.embedding.length > 0) {
                System.out.println("Storing " + record.id + ": " + record.content.length()
                    + " chars, " + record.embedding.length + " dims");
            }
        }
    }
}
```
