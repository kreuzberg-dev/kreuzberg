---
id: fixture_csharp_smoke_xlsx_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", Uri = "https://example.com/xlsx/stanley_cups.xlsx" }, new ExtractionConfig());
Console.WriteLine(result);

```
