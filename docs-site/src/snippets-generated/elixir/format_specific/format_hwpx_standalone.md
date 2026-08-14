---
id: fixture_elixir_format_hwpx_standalone
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{filename: "simple.hwpx", kind: "uri", mime_type: "application/haansofthwpx", uri: "https://example.com/hwpx/simple.hwpx"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
