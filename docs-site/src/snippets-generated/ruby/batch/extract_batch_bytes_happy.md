---
id: fixture_ruby_extract_batch_bytes_happy
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => [72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33], 'kind' => 'bytes', 'mime_type' => 'text/plain' }, { 'bytes' => 'test_documents/html/html.html', 'kind' => 'bytes', 'mime_type' => 'text/html' }])
puts result.inspect

```
