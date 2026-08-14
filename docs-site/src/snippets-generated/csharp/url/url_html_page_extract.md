---
id: fixture_csharp_url_html_page_extract
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com" }, new ExtractionConfig { Url = new UrlExtractionConfig { Mode = JsonSerializer.Deserialize<UrlExtractionMode>("\"document\"", ConfigOptions)! } });
Console.WriteLine(result);

```
