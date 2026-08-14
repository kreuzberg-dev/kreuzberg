---
id: fixture_elixir_config_pages
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{\"pages\":{\"extract_pages\":true,\"insert_page_markers\":true}}")
IO.inspect(result)

```
