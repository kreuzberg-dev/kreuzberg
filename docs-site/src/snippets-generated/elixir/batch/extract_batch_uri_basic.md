---
id: fixture_elixir_extract_batch_uri_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```elixir title="Elixir"
result = Xberg.extract_batch_async([%{"kind" => "uri", "uri" => "pdf/fake_memo.pdf"}, %{"kind" => "uri", "uri" => "text/fake_text.txt"}])
IO.inspect(result)

```
