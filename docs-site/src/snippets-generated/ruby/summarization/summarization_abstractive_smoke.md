---
id: fixture_ruby_summarization_abstractive_smoke
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/text/book_war_and_peace_1p.txt'), { 'summarization' => { 'llm' => { 'max_tokens' => 200, 'model' => 'openai/gpt-4o-mini', 'temperature' => 0.0 }, 'max_tokens' => 150, 'strategy' => 'abstractive' } })
puts result.inspect

```
