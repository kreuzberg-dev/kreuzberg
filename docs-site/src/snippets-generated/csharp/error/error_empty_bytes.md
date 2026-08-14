---
id: fixture_csharp_error_empty_bytes
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = new List<string> {  }, Config = new FileExtractionConfig(), Filename = "empty.txt", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "text/plain" }, new ExtractionConfig());
Console.WriteLine(result);

```
