---
id: fixture_ruby_extract_batch_uri_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => 'pdf/fake_memo.pdf' }, { 'kind' => 'uri', 'uri' => 'text/fake_text.txt' }])
puts result.inspect

```
