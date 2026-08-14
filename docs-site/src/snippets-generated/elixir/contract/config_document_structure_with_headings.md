---
id: fixture_elixir_config_document_structure_with_headings
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/docx/fake.docx"}
result = Xberg.extract_async(input_value, "{\"include_document_structure\":true}")
IO.inspect(result)

```
