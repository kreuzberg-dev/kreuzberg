---
id: fixture_csharp_output_format_bytes_markdown
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/pdf/fake_memo.pdf"), Config = new FileExtractionConfig { OutputFormat = OutputFormat.Markdown }, Filename = "fake_memo.pdf", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "application/pdf" }, new ExtractionConfig { OutputFormat = OutputFormat.Markdown });
Console.WriteLine(result);

```
