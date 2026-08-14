---
id: fixture_elixir_url_batch_mixed_inputs
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "https://example.com"}, %{"bytes" => [66, 97, 116, 99, 104, 32, 98, 121, 116, 101, 115, 32, 99, 111, 110, 116, 101, 110, 116], "filename" => "inline.txt", "kind" => "bytes", "mime_type" => "text/plain"}], "{\"url\":{\"mode\":\"document\"}}")
IO.inspect(result)

```
