using Xberg;

class Program
{
    static async Task Main()
    {
        try
        {
            var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), ExtractionConfig.Default())).Results[0];

            Console.WriteLine($"Content length: {result.Content.Length}");
            Console.WriteLine($"MIME type: {result.MimeType}");

            var tasks = new[]
            {
                XbergConverter.ExtractAsync(ExtractInput.FromUri("file1.pdf"), ExtractionConfig.Default()),
                XbergConverter.ExtractAsync(ExtractInput.FromUri("file2.pdf"), ExtractionConfig.Default()),
                XbergConverter.ExtractAsync(ExtractInput.FromUri("file3.pdf"), ExtractionConfig.Default())
            };

            var results = await Task.WhenAll(tasks);

            foreach (var r in results)
            {
                var document = r.Results[0];
                Console.WriteLine($"Extracted {document.Content.Length} characters");
            }
        }
        catch (XbergException ex)
        {
            Console.WriteLine($"Extraction failed: {ex.Message}");
        }
    }
}
