```csharp title="C#"
using Xberg;

var output = await XbergConverter.ExtractBatchAsync(
    new List<ExtractInput>
    {
        ExtractInput.FromUri("document.pdf"),
        ExtractInput.FromBytes(
            System.Text.Encoding.UTF8.GetBytes("Hello from memory"),
            "text/plain",
            "note.txt"
        ),
    },
    new ExtractionConfig()
);

foreach (var result in output.Results)
{
    Console.WriteLine(result.Content);
}
```
