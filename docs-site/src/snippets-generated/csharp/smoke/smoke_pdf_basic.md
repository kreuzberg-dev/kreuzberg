---
id: fixture_csharp_smoke_pdf_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/pdf", Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig());
Console.WriteLine(result);

```
