---
id: fixture_csharp_smoke_docx_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.wordprocessingml.document", Uri = "https://example.com/docx/fake.docx" }, new ExtractionConfig());
Console.WriteLine(result);

```
