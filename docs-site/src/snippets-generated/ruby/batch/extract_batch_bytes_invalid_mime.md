---
id: fixture_ruby_extract_batch_bytes_invalid_mime
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => [72, 101, 108, 108, 111], 'kind' => 'bytes', 'mime_type' => 'application/x-nonexistent' }])
puts result.inspect

```
