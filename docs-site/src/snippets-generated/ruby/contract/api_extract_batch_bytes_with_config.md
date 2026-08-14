---
id: fixture_ruby_api_extract_batch_bytes_with_config
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'bytes' => 'test_documents/pdf/fake_memo.pdf', 'config' => { 'output_format' => 'markdown' }, 'filename' => 'fake_memo.pdf', 'kind' => 'bytes' }])
puts result.inspect

```
