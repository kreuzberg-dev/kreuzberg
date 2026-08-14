---
id: fixture_csharp_api_extract_batch_uri
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
