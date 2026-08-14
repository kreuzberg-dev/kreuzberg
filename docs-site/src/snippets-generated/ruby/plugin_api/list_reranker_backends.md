---
id: fixture_ruby_list_reranker_backends
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```ruby title="Ruby"
require "xberg"
result = Xberg.list_reranker_backends()
puts result.inspect

```
