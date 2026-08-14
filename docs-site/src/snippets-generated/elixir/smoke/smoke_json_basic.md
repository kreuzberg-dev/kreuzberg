---
id: fixture_elixir_smoke_json_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "application/json", uri: "https://example.com/json/simple.json"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
