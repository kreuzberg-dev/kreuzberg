---
id: fixture_ruby_output_format_bytes_markdown
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/pdf/fake_memo.pdf').bytes, config: { 'output_format' => 'markdown' }, filename: 'fake_memo.pdf', kind: 'bytes', mime_type: 'application/pdf'), { 'output_format' => 'markdown' })
puts result.inspect

```
