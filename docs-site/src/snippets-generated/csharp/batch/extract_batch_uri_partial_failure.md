---
id: fixture_csharp_extract_batch_uri_partial_failure
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"text/plain.txt\"}", ConfigOptions)!, JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"/nonexistent/missing.pdf\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
