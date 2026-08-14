---
id: fixture_ruby_api_extract_batch_uri
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => 'https://example.com/pdf/fake_memo.pdf' }])
puts result.inspect

```
