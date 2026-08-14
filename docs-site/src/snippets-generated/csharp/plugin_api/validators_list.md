---
id: fixture_csharp_validators_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered validators

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListValidators();
Console.WriteLine(result);

```
