---
id: fixture_elixir_url_crawl_linked_pages
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com"}
result = Xberg.extract_async(input_value, "{\"url\":{\"crawl\":{\"max_depth\":1,\"max_pages\":4,\"respect_robots_txt\":false},\"mode\":\"crawl\"}}")
IO.inspect(result)

```
