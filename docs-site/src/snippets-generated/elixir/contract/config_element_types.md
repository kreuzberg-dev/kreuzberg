---
id: fixture_elixir_config_element_types
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/docx/unit_test_headers.docx"}
result = Xberg.extract_async(input_value, "{\"result_format\":\"element_based\"}")
IO.inspect(result)

```
