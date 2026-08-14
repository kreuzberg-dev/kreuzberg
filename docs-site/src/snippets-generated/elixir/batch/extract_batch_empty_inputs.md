---
id: fixture_elixir_extract_batch_empty_inputs
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```elixir title="Elixir"
result = Xberg.extract_batch_async([])
IO.inspect(result)

```
