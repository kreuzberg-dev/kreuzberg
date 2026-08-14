---
id: fixture_csharp_tokenizer_backends_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered tokenizer backends

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListTokenizerBackends();
Console.WriteLine(result);

```
