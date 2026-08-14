---
id: fixture_elixir_code_shebang_detection
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "text/x-source-code", uri: "https://example.com/code/script.sh"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
