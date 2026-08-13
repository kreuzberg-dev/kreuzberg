using Xberg;
using System.Collections.Generic;

class Program
{
    static async Task Main()
    {
        var config = new ExtractionConfig
        {
            UseCache = true,
            EnableQualityProcessing = true
        };

        var filePaths = new[]
        {
            "document1.pdf",
            "document2.pdf",
            "document3.pdf"
        };

        try
        {
            var batchResults = new List<ExtractionResult>();

            foreach (var filePath in filePaths)
            {
                var output = await XbergConverter.ExtractAsync(ExtractInput.FromUri(filePath), config);
                batchResults.Add(output);
                var document = output.Results[0];
                Console.WriteLine($"Processed {filePath}: {document.Content.Length} chars");
            }

            var tasks = filePaths.Select(path =>
                XbergConverter.ExtractAsync(ExtractInput.FromUri(path), config)
            ).ToArray();

            var results = await Task.WhenAll(tasks);

            var totalChars = results.Sum(output => output.Results.Sum(document => document.Content.Length));
            Console.WriteLine($"Total extracted: {totalChars} characters");
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Batch processing error: {ex.Message}");
        }
    }
}
