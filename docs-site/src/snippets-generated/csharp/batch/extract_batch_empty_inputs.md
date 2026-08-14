---
id: fixture_csharp_extract_batch_empty_inputs
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```csharp title="C#"
using System;
using Xberg;

var result = await XbergConverter.ExtractBatchAsync(new List<ExtractInput>() {  }, new ExtractionConfig());
Console.WriteLine(result);

```
