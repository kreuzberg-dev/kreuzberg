---
id: fixture_csharp_api_extract_bytes_input
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Bytes = System.IO.File.ReadAllBytes("test_documents/pdf/fake_memo.pdf"), Filename = "fake_memo.pdf", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"bytes\"", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
