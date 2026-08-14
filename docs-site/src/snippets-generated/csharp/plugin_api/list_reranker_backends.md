---
id: fixture_csharp_list_reranker_backends
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListRerankerBackends();
Console.WriteLine(result);

```
