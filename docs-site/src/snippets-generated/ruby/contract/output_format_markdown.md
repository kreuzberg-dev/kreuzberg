---
id: fixture_ruby_output_format_markdown
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'output_format' => 'markdown' })
puts result.inspect

```
