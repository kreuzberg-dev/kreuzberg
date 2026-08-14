---
id: fixture_elixir_config_tree_sitter
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/code/hello.py"}
result = Xberg.extract_async(input_value, "{\"tree_sitter\":{\"groups\":[\"web\"],\"languages\":[\"python\",\"rust\"],\"process\":{\"comments\":false,\"diagnostics\":false,\"docstrings\":false,\"exports\":true,\"imports\":true,\"structure\":true,\"symbols\":false}}}")
IO.inspect(result)

```
