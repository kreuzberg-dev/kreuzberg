---
id: fixture_ruby_config_keywords
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'keywords' => { 'algorithm' => 'yake', 'max_keywords' => 10 } })
puts result.inspect

```
