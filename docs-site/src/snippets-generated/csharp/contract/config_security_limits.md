---
id: fixture_csharp_config_security_limits
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/archives/documents.zip" }, new ExtractionConfig { SecurityLimits = new SecurityLimits { MaxArchiveSize = 104857600, MaxCompressionRatio = 50, MaxFilesInArchive = 100 } });
Console.WriteLine(result);

```
