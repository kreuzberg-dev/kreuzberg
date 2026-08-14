---
id: fixture_csharp_config_keywords
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { Keywords = new KeywordConfig { Algorithm = JsonSerializer.Deserialize<KeywordAlgorithm>("\"yake\"", ConfigOptions)!, MaxKeywords = 10 } });
Console.WriteLine(result);

```
