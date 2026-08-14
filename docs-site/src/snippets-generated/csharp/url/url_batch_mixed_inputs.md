---
id: fixture_csharp_url_batch_mixed_inputs
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() { JsonSerializer.Deserialize<ExtractInput>("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ConfigOptions)!, JsonSerializer.Deserialize<ExtractInput>("{\"bytes\":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],\"filename\":\"inline.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ConfigOptions)! }, new ExtractionConfig { Url = new UrlExtractionConfig { Mode = JsonSerializer.Deserialize<UrlExtractionMode>("\"document\"", ConfigOptions)! } });
Console.WriteLine(result);

```
