---
id: fixture_csharp_smoke_html_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "text/html", Uri = "https://example.com/html/simple_table.html" }, new ExtractionConfig());
Console.WriteLine(result);

```
