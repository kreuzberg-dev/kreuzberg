---
id: fixture_csharp_config_llm_structured_extraction
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/pdf/fake_memo.pdf" }, new ExtractionConfig { StructuredExtraction = new StructuredExtractionConfig { Llm = new LlmConfig { Model = "openai/gpt-4o" }, Schema = "{\"properties\":{\"date\":{\"type\":\"string\"},\"summary\":{\"type\":\"string\"},\"title\":{\"type\":\"string\"}},\"required\":[\"title\"],\"type\":\"object\"}", SchemaName = "memo_data" } });
Console.WriteLine(result);

```
