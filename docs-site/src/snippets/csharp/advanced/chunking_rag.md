```csharp title="C#"
using Xberg;
using System.Collections.Generic;
using System.Linq;

class RagPipelineExample
{
    static async Task Main()
    {
        var config = new ExtractionConfig
        {
            Chunking = new ChunkingConfig
            {
                MaxCharacters = 500,
                Overlap = 50,
                Embedding = new EmbeddingConfig
                {
                    Model = new EmbeddingModelType.Preset("all-mpnet-base-v2"),
                    Normalize = true,
                    BatchSize = 16
                }
            }
        };

        try
        {
            var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri(
                "research_paper.pdf"), config
            )).Results[0];

            var vectorStore = await BuildVectorStoreAsync(result.Chunks ?? new List<Chunk>())
                .ConfigureAwait(false);

            var query = "machine learning optimization";
            var relevantChunks = await SearchAsync(vectorStore, query)
                .ConfigureAwait(false);

            Console.WriteLine($"Found {relevantChunks.Count} relevant chunks");
            foreach (var chunk in relevantChunks.Take(3))
            {
                Console.WriteLine($"Content: {chunk.Content[..80]}...");
                Console.WriteLine($"Similarity: {chunk.Similarity:F3}\n");
            }
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Error: {ex.Message}");
        }
    }

    static async Task<List<VectorEntry>> BuildVectorStoreAsync(
        IEnumerable<Chunk> chunks)
    {
        return await Task.Run(() =>
        {
            return chunks.Select(c => new VectorEntry
            {
                Content = c.Content,
                Embedding = c.Embedding?.ToArray() ?? Array.Empty<float>(),
                Similarity = 0f
            }).ToList();
        }).ConfigureAwait(false);
    }

    static async Task<List<VectorEntry>> SearchAsync(
        List<VectorEntry> store,
        string query)
    {
        return await Task.Run(() =>
        {
            return store
                .OrderByDescending(e => e.Similarity)
                .ToList();
        }).ConfigureAwait(false);
    }

    class VectorEntry
    {
        public string Content { get; set; } = string.Empty;
        public float[] Embedding { get; set; } = Array.Empty<float>();
        public float Similarity { get; set; }
    }
}
```
