---
id: fixture_elixir_extract_batch_uri_all_missing
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "/nonexistent/a.pdf"}, %{"kind" => "uri", "uri" => "/nonexistent/b.txt"}])
IO.inspect(result)

```
