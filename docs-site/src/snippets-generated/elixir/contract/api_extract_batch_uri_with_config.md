---
id: fixture_elixir_api_extract_batch_uri_with_config
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"config" => %{"output_format" => "markdown"}, "kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"}])
IO.inspect(result)

```
