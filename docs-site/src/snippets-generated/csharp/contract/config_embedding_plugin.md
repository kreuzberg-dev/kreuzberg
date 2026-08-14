---
id: fixture_csharp_config_embedding_plugin
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { Chunking = new ChunkingConfig { Embedding = new EmbeddingConfig { MaxEmbedDurationSecs = 30, Model = JsonSerializer.Deserialize<EmbeddingModelType>("{\"name\":\"test-plugin-backend\",\"type\":\"plugin\"}", ConfigOptions)!, Normalize = true }, MaxChars = 500, MaxOverlap = 50 } });
Console.WriteLine(result);

```
