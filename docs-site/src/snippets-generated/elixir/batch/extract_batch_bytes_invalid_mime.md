---
id: fixture_elixir_extract_batch_bytes_invalid_mime
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"bytes" => [72, 101, 108, 108, 111], "kind" => "bytes", "mime_type" => "application/x-nonexistent"}])
IO.inspect(result)

```
