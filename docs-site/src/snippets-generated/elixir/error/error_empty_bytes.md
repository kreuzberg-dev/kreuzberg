---
id: fixture_elixir_error_empty_bytes
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{bytes: [], config: %{}, filename: "empty.txt", kind: "bytes", mime_type: "text/plain"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
