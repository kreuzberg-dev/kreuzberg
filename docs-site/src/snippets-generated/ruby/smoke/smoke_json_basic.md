---
id: fixture_ruby_smoke_json_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/json', uri: 'https://example.com/json/simple.json'), {  })
puts result.inspect

```
