---
id: fixture_elixir_config_keywords
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{\"keywords\":{\"algorithm\":\"yake\",\"max_keywords\":10}}")
IO.inspect(result)

```
