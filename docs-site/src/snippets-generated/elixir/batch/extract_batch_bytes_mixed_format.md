---
id: fixture_elixir_extract_batch_bytes_mixed_format
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"bytes" => [80, 68, 70, 32, 112, 108, 97, 99, 101, 104, 111, 108, 100, 101, 114], "kind" => "bytes", "mime_type" => "application/x-unknown"}])
IO.inspect(result)

```
