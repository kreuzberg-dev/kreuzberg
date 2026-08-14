---
id: fixture_csharp_list_ocr_backends
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```csharp title="C#"
using System;
using Xberg;

var result = XbergConverter.ListOcrBackends();
Console.WriteLine(result);

```
