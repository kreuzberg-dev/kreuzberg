---
id: fixture_elixir_extract_batch_uri_not_found
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "/nonexistent/a.pdf"}])
IO.inspect(result)

```
