---
id: fixture_csharp_extract_batch_bytes_size_cap
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
try
{
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":\"test_documents/text/fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ConfigOptions)! }, new ExtractionConfig { SecurityLimits = new SecurityLimits { MaxContentSize = 1 } });
}
catch (Exception error)
{
    Console.Error.WriteLine($"{error.GetType().Name}: {error.Message}");
}

```
