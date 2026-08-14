---
id: fixture_csharp_config_element_types
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/docx/unit_test_headers.docx" }, new ExtractionConfig { ResultFormat = JsonSerializer.Deserialize<ResultFormat>("\"element_based\"", ConfigOptions)! });
Console.WriteLine(result);

```
