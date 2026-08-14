---
id: fixture_elixir_summarization_extractive_smoke
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/text/book_war_and_peace_1p.txt"}
result = Xberg.extract_async(input_value, "{\"summarization\":{\"max_tokens\":80,\"strategy\":\"extractive\"}}")
IO.inspect(result)

```
