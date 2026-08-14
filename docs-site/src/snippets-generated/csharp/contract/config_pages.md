---
id: fixture_csharp_config_pages
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { Pages = new PageConfig { ExtractPages = true, InsertPageMarkers = true } });
Console.WriteLine(result);

```
