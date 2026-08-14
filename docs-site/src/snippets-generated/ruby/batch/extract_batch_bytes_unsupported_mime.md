---
id: fixture_ruby_extract_batch_bytes_unsupported_mime
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => [100, 97, 116, 97], 'kind' => 'bytes', 'mime_type' => 'application/x-unknown' }])
puts result.inspect

```
