---
id: fixture_ruby_extract_batch_uri_not_found
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => '/nonexistent/a.pdf' }])
puts result.inspect

```
