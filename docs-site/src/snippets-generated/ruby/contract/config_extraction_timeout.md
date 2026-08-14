---
id: fixture_ruby_config_extraction_timeout
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'extraction_timeout_secs' => 300 })
puts result.inspect

```
