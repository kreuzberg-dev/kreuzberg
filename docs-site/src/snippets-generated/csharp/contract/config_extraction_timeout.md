---
id: fixture_csharp_config_extraction_timeout
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { ExtractionTimeoutSecs = 300 });
Console.WriteLine(result);

```
