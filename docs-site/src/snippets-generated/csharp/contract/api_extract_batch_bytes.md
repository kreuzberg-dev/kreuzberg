---
id: fixture_csharp_api_extract_batch_bytes
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction API (extract_batch)

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":\"test_documents/pdf/fake_memo.pdf\",\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
