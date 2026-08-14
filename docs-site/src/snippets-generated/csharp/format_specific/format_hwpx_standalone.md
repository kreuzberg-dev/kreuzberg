---
id: fixture_csharp_format_hwpx_standalone
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Filename = "simple.hwpx", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/haansofthwpx", Uri = "https://example.com/hwpx/simple.hwpx" }, new ExtractionConfig());
Console.WriteLine(result);

```
