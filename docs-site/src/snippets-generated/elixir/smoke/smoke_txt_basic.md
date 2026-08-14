---
id: fixture_elixir_smoke_txt_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "text/plain", uri: "https://example.com/text/report.txt"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
