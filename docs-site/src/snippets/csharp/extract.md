```csharp title="C#"
using Xberg;

var output = await XbergConverter.ExtractAsync(
    ExtractInput.FromUri("document.pdf"),
    new ExtractionConfig()
);

Console.WriteLine(output.Results[0].Content);
```
