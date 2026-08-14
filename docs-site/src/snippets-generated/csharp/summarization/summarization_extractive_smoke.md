---
id: fixture_csharp_summarization_extractive_smoke
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/text/book_war_and_peace_1p.txt" }, new ExtractionConfig { Summarization = new SummarizationConfig { MaxTokens = 80, Strategy = JsonSerializer.Deserialize<SummaryStrategy>("\"extractive\"", ConfigOptions)! } });
Console.WriteLine(result);

```
