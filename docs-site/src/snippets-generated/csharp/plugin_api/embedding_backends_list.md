---
id: fixture_csharp_embedding_backends_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListEmbeddingBackends();
Console.WriteLine(result);

```
