```csharp title="C#"
using Xberg;

var config = new ExtractionConfig
{
    TokenReduction = new TokenReductionOptions
    {
        Mode = "moderate",
        PreserveImportantWords = true
    }
};

var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri(
    "verbose_document.pdf"), config
)).Results[0];

Console.WriteLine($"Reduced content length: {result.Content.Length} chars");
```
