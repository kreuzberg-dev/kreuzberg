---
id: fixture_elixir_api_extract_uri
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
