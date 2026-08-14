---
id: fixture_csharp_smoke_json_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/json", Uri = "https://example.com/json/simple.json" }, new ExtractionConfig());
Console.WriteLine(result);

```
