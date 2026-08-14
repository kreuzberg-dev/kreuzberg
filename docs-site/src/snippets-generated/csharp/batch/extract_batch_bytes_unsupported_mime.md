---
id: fixture_csharp_extract_batch_bytes_unsupported_mime
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":[100,97,116,97],\"kind\":\"bytes\",\"mime_type\":\"application/x-unknown\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
