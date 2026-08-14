```csharp title="C#"
using Xberg;

try
{
    var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("missing.pdf"), new ExtractionConfig())).Results[0];
    Console.WriteLine(result.Content);
}
catch (ValidationException ex)
{
    Console.Error.WriteLine($"Validation error: {ex.Message}");
}
catch (IoException ex)
{
    Console.Error.WriteLine($"IO error: {ex.Message}");
    throw;
}
catch (XbergException ex)
{
    Console.Error.WriteLine($"Extraction failed: {ex.Message}");
    throw;
}
```
