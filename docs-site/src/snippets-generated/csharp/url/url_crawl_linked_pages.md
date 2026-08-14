---
id: fixture_csharp_url_crawl_linked_pages
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com" }, new ExtractionConfig { Url = new UrlExtractionConfig { Crawl = new CrawlConfig { MaxDepth = 1, MaxPages = 4, RespectRobotsTxt = false }, Mode = JsonSerializer.Deserialize<UrlExtractionMode>("\"crawl\"", ConfigOptions)! } });
Console.WriteLine(result);

```
