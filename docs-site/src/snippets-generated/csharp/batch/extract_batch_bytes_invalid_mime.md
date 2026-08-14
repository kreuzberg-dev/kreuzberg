---
id: fixture_csharp_extract_batch_bytes_invalid_mime
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":[72,101,108,108,111],\"kind\":\"bytes\",\"mime_type\":\"application/x-nonexistent\"}", ConfigOptions)! }, new ExtractionConfig());
Console.WriteLine(result);

```
