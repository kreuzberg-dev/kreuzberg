---
id: fixture_ruby_config_quality_enabled
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'enable_quality_processing' => true })
puts result.inspect

```
