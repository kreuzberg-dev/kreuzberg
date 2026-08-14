---
id: fixture_csharp_extract_batch_bytes_happy
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":[72,101,108,108,111,44,32,119,111,114,108,100,33],\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ConfigOptions)!, JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":\"test_documents/html/html.html\",\"kind\":\"bytes\",\"mime_type\":\"text/html\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
