---
id: fixture_elixir_smoke_html_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "text/html", uri: "https://example.com/html/simple_table.html"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
