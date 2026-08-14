---
id: fixture_csharp_error_unsupported_mime
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with unsupported MIME type

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
try
{
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/text/plain.txt"), Config = new FileExtractionConfig(), Filename = "plain.txt", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "application/x-nonexistent" }, new ExtractionConfig());
}
catch (Exception error)
{
    Console.Error.WriteLine($"{error.GetType().Name}: {error.Message}");
}

```
