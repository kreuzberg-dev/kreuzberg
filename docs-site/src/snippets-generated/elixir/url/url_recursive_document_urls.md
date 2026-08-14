---
id: fixture_elixir_url_recursive_document_urls
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com"}
result = Xberg.extract_async(input_value, "{\"url\":{\"crawl\":{\"document_url_depth\":1,\"follow_document_urls\":true,\"respect_robots_txt\":false},\"mode\":\"document\"}}")
IO.inspect(result)

```
