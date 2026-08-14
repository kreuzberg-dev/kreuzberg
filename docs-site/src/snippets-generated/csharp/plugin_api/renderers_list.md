---
id: fixture_csharp_renderers_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered renderers

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListRenderers();
Console.WriteLine(result);

```
