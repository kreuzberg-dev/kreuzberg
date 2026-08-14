---
id: fixture_csharp_format_pptx
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.presentationml.presentation", Uri = "https://example.com/pptx/simple.pptx" }, new ExtractionConfig());
Console.WriteLine(result);

```
