---
id: fixture_ruby_embedding_backends_list
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```ruby title="Ruby"
require "xberg"
result = Xberg.list_embedding_backends()
puts result.inspect

```
