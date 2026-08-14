---
id: fixture_ruby_api_extract_bytes_input
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/pdf/fake_memo.pdf').bytes, filename: 'fake_memo.pdf', kind: 'bytes'))
puts result.inspect

```
