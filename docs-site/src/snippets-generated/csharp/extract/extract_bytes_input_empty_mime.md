---
id: fixture_csharp_extract_bytes_input_empty_mime
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with empty MIME type

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
try
{
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/text/plain.txt"), Config = new FileExtractionConfig(), Filename = "plain.txt", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "" }, new ExtractionConfig());
}
catch (Exception error)
{
    Console.Error.WriteLine($"Call failed as expected: {error.Message}");
    return;
}
throw new InvalidOperationException("expected call to fail");

```
