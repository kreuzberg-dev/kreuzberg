---
id: fixture_elixir_config_extraction_timeout
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{\"extraction_timeout_secs\":300}")
IO.inspect(result)

```
