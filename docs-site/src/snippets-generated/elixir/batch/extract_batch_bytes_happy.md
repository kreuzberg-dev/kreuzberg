---
id: fixture_elixir_extract_batch_bytes_happy
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"bytes" => [72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33], "kind" => "bytes", "mime_type" => "text/plain"}, %{"bytes" => "test_documents/html/html.html", "kind" => "bytes", "mime_type" => "text/html"}])
IO.inspect(result)

```
