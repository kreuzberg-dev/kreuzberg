---
id: fixture_csharp_config_tree_sitter
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```csharp title="C#"
using System;
using System.Text.Json;
using Xberg;

var ConfigOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
var result = await XbergConverter.ExtractAsync(new ExtractInput { Kind = JsonSerializer.Deserialize<ExtractInputKind>("\"uri\"", ConfigOptions)!, Uri = "https://example.com/code/hello.py" }, new ExtractionConfig { TreeSitter = new TreeSitterConfig { Groups = new List<string> { "web" }, Languages = new List<string> { "python", "rust" }, Process = new TreeSitterProcessConfig { Comments = false, Diagnostics = false, Docstrings = false, Exports = true, Imports = true, Structure = true, Symbols = false } } });
Console.WriteLine(result);

```
