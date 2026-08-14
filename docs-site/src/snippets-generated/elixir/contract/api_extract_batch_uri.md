---
id: fixture_elixir_api_extract_batch_uri
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"}])
IO.inspect(result)

```
