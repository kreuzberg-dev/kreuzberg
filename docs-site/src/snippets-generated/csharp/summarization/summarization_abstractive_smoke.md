---
id: fixture_csharp_summarization_abstractive_smoke
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/text/book_war_and_peace_1p.txt" }, new ExtractionConfig { Summarization = new SummarizationConfig { Llm = new LlmConfig { MaxTokens = 200, Model = "openai/gpt-4o-mini", Temperature = 0.0d }, MaxTokens = 150, Strategy = JsonSerializer.Deserialize<SummaryStrategy>("\"abstractive\"", ConfigOptions)! } });
Console.WriteLine(result);

```
