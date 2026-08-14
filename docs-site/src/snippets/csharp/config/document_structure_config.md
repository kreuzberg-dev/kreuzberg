```csharp title="Document Structure Config (C#)"
using Xberg;

var config = new ExtractionConfig
{
    IncludeDocumentStructure = true
};

var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), config)).Results[0];

if (result.Document is not null)
{
    foreach (var node in result.Document.Nodes)
    {
        Console.WriteLine($"[{node.Content.GetType().Name}]");
    }
}
```
