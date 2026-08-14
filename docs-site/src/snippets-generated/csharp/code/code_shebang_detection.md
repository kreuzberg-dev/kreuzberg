---
id: fixture_csharp_code_shebang_detection
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "text/x-source-code", Uri = "https://example.com/code/script.sh" }, new ExtractionConfig());
Console.WriteLine(result);

```
