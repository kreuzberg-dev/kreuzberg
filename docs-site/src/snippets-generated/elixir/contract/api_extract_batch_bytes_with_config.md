---
id: fixture_elixir_api_extract_batch_bytes_with_config
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"bytes" => "test_documents/pdf/fake_memo.pdf", "config" => %{"output_format" => "markdown"}, "filename" => "fake_memo.pdf", "kind" => "bytes"}])
IO.inspect(result)

```
