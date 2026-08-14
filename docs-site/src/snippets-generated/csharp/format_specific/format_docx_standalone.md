---
id: fixture_csharp_format_docx_standalone
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Filename = "fake.docx", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.wordprocessingml.document", Uri = "https://example.com/docx/fake.docx" }, new ExtractionConfig());
Console.WriteLine(result);

```
