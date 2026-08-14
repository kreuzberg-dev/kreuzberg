---
id: fixture_ruby_error_empty_bytes
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(bytes: [], config: {  }, filename: 'empty.txt', kind: 'bytes', mime_type: 'text/plain'), {  })
puts result.inspect

```
