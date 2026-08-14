---
id: fixture_ruby_extract_batch_empty_inputs
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([])
puts result.inspect

```
