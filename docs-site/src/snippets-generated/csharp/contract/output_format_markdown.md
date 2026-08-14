---
id: fixture_csharp_output_format_markdown
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { OutputFormat = OutputFormat.Markdown });
Console.WriteLine(result);

```
