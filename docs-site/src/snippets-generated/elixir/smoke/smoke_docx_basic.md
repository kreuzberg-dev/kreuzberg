---
id: fixture_elixir_smoke_docx_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri: "https://example.com/docx/fake.docx"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
