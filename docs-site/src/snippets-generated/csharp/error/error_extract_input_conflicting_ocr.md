---
id: fixture_csharp_error_extract_input_conflicting_ocr
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
try
{
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/text/fake_text.txt"), Config = new FileExtractionConfig { DisableOcr = true, ForceOcr = true }, Filename = "fake_text.txt", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "text/plain" }, new ExtractionConfig { DisableOcr = true, ForceOcr = true });
}
catch (Exception error)
{
    Console.Error.WriteLine($"Call failed as expected: {error.Message}");
    return;
}
throw new InvalidOperationException("expected call to fail");

```
