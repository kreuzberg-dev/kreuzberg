---
id: fixture_elixir_format_docx_standalone
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{filename: "fake.docx", kind: "uri", mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/fake.docx"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
