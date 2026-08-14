---
id: fixture_elixir_format_pptx
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation", uri: "https://example.com/pptx/simple.pptx"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
