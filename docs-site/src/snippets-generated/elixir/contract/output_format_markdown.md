---
id: fixture_elixir_output_format_markdown
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{\"output_format\":\"markdown\"}")
IO.inspect(result)

```
