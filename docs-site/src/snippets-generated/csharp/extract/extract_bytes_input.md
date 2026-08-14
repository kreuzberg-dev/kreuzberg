---
id: fixture_csharp_extract_bytes_input
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/pdf/fake_memo.pdf"), Filename = "fake_memo.pdf", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)!, MimeType = "application/pdf" }, new ExtractionConfig());
Console.WriteLine(result);

```
