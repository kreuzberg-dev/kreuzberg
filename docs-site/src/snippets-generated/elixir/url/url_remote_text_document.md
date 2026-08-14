---
id: fixture_elixir_url_remote_text_document
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com"}
result = Xberg.extract_async(input_value, "{\"url\":{\"mode\":\"document\"}}")
IO.inspect(result)

```
