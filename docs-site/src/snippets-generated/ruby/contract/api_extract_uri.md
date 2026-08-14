---
id: fixture_ruby_api_extract_uri
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'))
puts result.inspect

```
