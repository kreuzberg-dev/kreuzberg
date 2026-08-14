---
id: fixture_elixir_format_docx_equations
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{filename: "equations.docx", kind: "uri", mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/equations.docx"}
result = Xberg.extract_async(input_value, "{\"output_format\":\"markdown\"}")
IO.inspect(result)

```
