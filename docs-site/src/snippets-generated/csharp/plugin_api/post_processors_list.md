---
id: fixture_csharp_post_processors_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered post-processors

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListPostProcessors();
Console.WriteLine(result);

```
