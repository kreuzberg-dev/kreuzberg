---
id: fixture_ruby_url_batch_mixed_inputs
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => 'https://example.com' }, { 'bytes' => [66, 97, 116, 99, 104, 32, 98, 121, 116, 101, 115, 32, 99, 111, 110, 116, 101, 110, 116], 'filename' => 'inline.txt', 'kind' => 'bytes', 'mime_type' => 'text/plain' }], { 'url' => { 'mode' => 'document' } })
puts result.inspect

```
