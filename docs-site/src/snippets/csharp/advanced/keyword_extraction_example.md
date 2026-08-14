```csharp title="C#"
using Xberg;
using System.Collections.Generic;

var config = new ExtractionConfig
{
    Keywords = new KeywordConfig
    {
        Algorithm = KeywordAlgorithm.Yake,
        MaxKeywords = 10,
        MinScore = 0.3f
    }
};

var output = await XbergConverter.ExtractAsync(
    ExtractInput.FromUri("research_paper.pdf"),
    config
);
var result = output.Results[0];

foreach (var keyword in result.ExtractedKeywords ?? new List<Keyword>())
{
    Console.WriteLine($"{keyword.Text}: {keyword.Score:F3}");
}
```
