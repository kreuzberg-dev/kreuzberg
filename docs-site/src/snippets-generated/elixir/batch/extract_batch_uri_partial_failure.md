---
id: fixture_elixir_extract_batch_uri_partial_failure
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "text/plain.txt"}, %{"kind" => "uri", "uri" => "/nonexistent/missing.pdf"}])
IO.inspect(result)

```
