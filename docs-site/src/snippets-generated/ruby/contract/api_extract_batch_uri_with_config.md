---
id: fixture_ruby_api_extract_batch_uri_with_config
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'config' => { 'output_format' => 'markdown' }, 'kind' => 'uri', 'uri' => 'https://example.com/pdf/fake_memo.pdf' }])
puts result.inspect

```
