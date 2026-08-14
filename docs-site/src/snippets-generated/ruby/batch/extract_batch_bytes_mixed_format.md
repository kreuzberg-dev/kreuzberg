---
id: fixture_ruby_extract_batch_bytes_mixed_format
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => [80, 68, 70, 32, 112, 108, 97, 99, 101, 104, 111, 108, 100, 101, 114], 'kind' => 'bytes', 'mime_type' => 'application/x-unknown' }])
puts result.inspect

```
