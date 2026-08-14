---
id: fixture_csharp_ocr_backends_list
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List all registered OCR backends

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListOcrBackends();
Console.WriteLine(result);

```
