---
id: fixture_ruby_summarization_extractive_smoke
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/text/book_war_and_peace_1p.txt'), { 'summarization' => { 'max_tokens' => 80, 'strategy' => 'extractive' } })
puts result.inspect

```
