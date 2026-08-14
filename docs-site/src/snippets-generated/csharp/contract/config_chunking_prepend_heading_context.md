---
id: fixture_csharp_config_chunking_prepend_heading_context
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "document.md" }, new ExtractionConfig { Chunking = new ChunkingConfig { ChunkerType = JsonSerializer.Deserialize<ChunkerType>("\"markdown\"", ConfigOptions)!, MaxCharacters = 500, Overlap = 50, PrependHeadingContext = true } });
Console.WriteLine(result);

```
