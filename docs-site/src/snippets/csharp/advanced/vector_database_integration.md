```csharp title="C#"
using Xberg;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;

var integration = new VectorDatabaseIntegration();
var records = await integration.ExtractAndVectorize("research_paper.pdf", "doc-1");
Console.WriteLine($"Vectorized {records.Count} chunks");

public class VectorDatabaseIntegration
{
    public class VectorRecord
    {
        public string Id { get; set; } = string.Empty;
        public float[] Embedding { get; set; } = Array.Empty<float>();
        public string Content { get; set; } = string.Empty;
        public Dictionary<string, string> Metadata { get; set; } = new();
    }

    public async Task<List<VectorRecord>> ExtractAndVectorize(
        string documentPath,
        string documentId)
    {
        var config = new ExtractionConfig
        {
            Chunking = new ChunkingConfig
            {
                MaxCharacters = 512,
                Overlap = 50,
                Embedding = new EmbeddingConfig
                {
                    Model = new EmbeddingModelType.Preset("balanced"),
                    Normalize = true,
                    BatchSize = 32
                }
            }
        };

        var result = await XbergConverter.ExtractAsync(ExtractInput.FromUri(documentPath), config);
        var chunks = result.Results[0].Chunks ?? new List<Chunk>();

        var vectorRecords = chunks
            .Select((chunk, index) => new VectorRecord
            {
                Id = $"{documentId}_chunk_{index}",
                Content = chunk.Content,
                Embedding = chunk.Embedding?.ToArray() ?? Array.Empty<float>(),
                Metadata = new Dictionary<string, string>
                {
                    { "document_id", documentId },
                    { "chunk_index", index.ToString() },
                    { "content_length", chunk.Content.Length.ToString() }
                }
            })
            .ToList();

        await StoreInVectorDatabase(vectorRecords);
        return vectorRecords;
    }

    private async Task StoreInVectorDatabase(List<VectorRecord> records)
    {
        foreach (var record in records)
        {
            if (record.Embedding != null && record.Embedding.Length > 0)
            {
                Console.WriteLine(
                    $"Storing {record.Id}: {record.Content.Length} chars, " +
                    $"{record.Embedding.Length} dims");
            }
        }

        await Task.CompletedTask;
    }
}
```
