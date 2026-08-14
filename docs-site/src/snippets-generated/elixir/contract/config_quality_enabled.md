---
id: fixture_elixir_config_quality_enabled
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{\"enable_quality_processing\":true}")
IO.inspect(result)

```
