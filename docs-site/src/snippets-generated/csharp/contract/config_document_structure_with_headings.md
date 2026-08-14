---
id: fixture_csharp_config_document_structure_with_headings
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/docx/fake.docx" }, new ExtractionConfig { IncludeDocumentStructure = true });
Console.WriteLine(result);

```
