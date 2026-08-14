---
id: fixture_elixir_extract_batch_bytes_unsupported_mime
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"bytes" => [100, 97, 116, 97], "kind" => "bytes", "mime_type" => "application/x-unknown"}])
IO.inspect(result)

```
