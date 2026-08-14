---
id: fixture_csharp_extract_batch_bytes_mixed_format
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":[80,68,70,32,112,108,97,99,101,104,111,108,100,101,114],\"kind\":\"bytes\",\"mime_type\":\"application/x-unknown\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
