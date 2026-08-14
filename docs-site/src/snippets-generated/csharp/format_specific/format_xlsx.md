---
id: fixture_csharp_format_xlsx
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", Uri = "https://example.com/xlsx/stanley_cups.xlsx" }, new ExtractionConfig());
Console.WriteLine(result);

```
