---
id: fixture_csharp_url_recursive_document_urls
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com" }, new ExtractionConfig { Url = new UrlExtractionConfig { Crawl = new CrawlConfig { DocumentUrlDepth = 1, FollowDocumentUrls = true, RespectRobotsTxt = false }, Mode = JsonSerializer.Deserialize<UrlExtractionMode>("\"document\"", ConfigOptions)! } });
Console.WriteLine(result);

```
