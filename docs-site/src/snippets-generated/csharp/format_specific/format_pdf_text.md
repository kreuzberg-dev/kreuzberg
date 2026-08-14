---
id: fixture_csharp_format_pdf_text
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Filename = "fake_memo.pdf", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/pdf", Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig());
Console.WriteLine(result);

```
