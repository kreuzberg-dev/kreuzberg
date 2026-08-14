---
id: fixture_csharp_smoke_txt_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "text/plain", Uri = "https://example.com/text/report.txt" }, new ExtractionConfig());
Console.WriteLine(result);

```
