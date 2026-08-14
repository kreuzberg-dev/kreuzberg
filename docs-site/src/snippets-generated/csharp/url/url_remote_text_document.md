---
id: fixture_csharp_url_remote_text_document
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com" }, new ExtractionConfig { Url = new UrlExtractionConfig { Mode = JsonSerializer.Deserialize<UrlExtractionMode>("\"document\"", ConfigOptions)! } });
Console.WriteLine(result);

```
