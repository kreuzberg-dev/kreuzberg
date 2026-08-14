---
id: fixture_csharp_format_docx_equations
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```csharp title="C#"
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Filename = "equations.docx", Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, MimeType = "application/vnd.openxmlformats-officedocument.wordprocessingml.document", Uri = "https://example.com/docx/equations.docx" }, new ExtractionConfig { OutputFormat = OutputFormat.Markdown });

```
