---
id: fixture_elixir_config_chunking_prepend_heading_context
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "document.md"}
result = Xberg.extract_async(input_value, "{\"chunking\":{\"chunker_type\":\"markdown\",\"max_characters\":500,\"overlap\":50,\"prepend_heading_context\":true}}")
IO.inspect(result)

```
