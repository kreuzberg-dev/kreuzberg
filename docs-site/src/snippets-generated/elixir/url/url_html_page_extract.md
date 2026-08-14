---
id: fixture_elixir_url_html_page_extract
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com"}
result = Xberg.extract_async(input_value, "{\"url\":{\"mode\":\"document\"}}")
IO.inspect(result)

```
