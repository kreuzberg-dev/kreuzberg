---
id: fixture_ruby_extract_bytes_input
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/pdf/fake_memo.pdf').bytes, filename: 'fake_memo.pdf', kind: 'bytes', mime_type: 'application/pdf'))
puts result.inspect

```
