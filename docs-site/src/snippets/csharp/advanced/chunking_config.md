```csharp title="C#"
using Xberg;

class Program
{
    static async Task Main()
    {
        var config = new ExtractionConfig
        {
            Chunking = new ChunkingConfig
            {
                MaxCharacters = 1000,
                Overlap = 200,
                Embedding = new EmbeddingConfig
                {
                    Model = new EmbeddingModelType.Preset("all-minilm-l6-v2"),
                    Normalize = true,
                    BatchSize = 32
                }
            }
        };

        try
        {
            var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri(
                "document.pdf"), config
            )).Results[0];

            // Chunks is null unless chunking is configured, as it is above.
            var chunks = result.Chunks ?? [];

            Console.WriteLine($"Chunks: {chunks.Count}");
            foreach (var chunk in chunks)
            {
                Console.WriteLine($"Content length: {chunk.Content.Length}");
                if (chunk.Embedding != null)
                {
                    Console.WriteLine($"Embedding dimensions: {chunk.Embedding.Count}");
                }
            }
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Error: {ex.Message}");
        }
    }
}
```

```csharp title="C# - Markdown with Heading Context"
using Xberg;

class Program
{
    static async Task Main()
    {
        var config = new ExtractionConfig
        {
            Chunking = new ChunkingConfig
            {
                MaxCharacters = 500,
                Overlap = 50,
                Sizing = new ChunkSizing.Tokenizer("Xenova/gpt-4o", null)
            }
        };

        try
        {
            var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri(
                "document.md"), config
            )).Results[0];

            foreach (var chunk in result.Chunks ?? [])
            {
                // Heading context lives on the chunk's metadata.
                var headingContext = chunk.Metadata.HeadingContext;
                if (headingContext != null)
                {
                    Console.WriteLine("Headings:");
                    foreach (var heading in headingContext.Headings)
                    {
                        Console.WriteLine($"  Level {heading.Level}: {heading.Text}");
                    }
                }
            }
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Error: {ex.Message}");
        }
    }
}
```

```csharp title="C# - Prepend Heading Context"
using Xberg;

class Program
{
    static async Task Main()
    {
        var config = new ExtractionConfig
        {
            Chunking = new ChunkingConfig
            {
                MaxCharacters = 500,
                Overlap = 50,
                PrependHeadingContext = true
            }
        };

        try
        {
            var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri(
                "document.md"), config
            )).Results[0];

            foreach (var chunk in result.Chunks ?? [])
            {
                // Each chunk's content is prefixed with its heading breadcrumb
                Console.WriteLine(chunk.Content[..Math.Min(100, chunk.Content.Length)]);
            }
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Error: {ex.Message}");
        }
    }
}
```
