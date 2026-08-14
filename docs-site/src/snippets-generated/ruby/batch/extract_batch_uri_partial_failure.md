---
id: fixture_ruby_extract_batch_uri_partial_failure
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => 'text/plain.txt' }, { 'kind' => 'uri', 'uri' => '/nonexistent/missing.pdf' }])
puts result.inspect

```
