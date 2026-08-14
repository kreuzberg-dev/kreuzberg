---
id: fixture_ruby_api_extract_batch_bytes
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction API (extract_batch)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => 'test_documents/pdf/fake_memo.pdf', 'filename' => 'fake_memo.pdf', 'kind' => 'bytes' }])
puts result.inspect

```
