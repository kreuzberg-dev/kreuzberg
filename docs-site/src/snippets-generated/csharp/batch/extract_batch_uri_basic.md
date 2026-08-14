---
id: fixture_csharp_extract_batch_uri_basic
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"pdf/fake_memo.pdf\"}", ConfigOptions)!, JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"text/fake_text.txt\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
