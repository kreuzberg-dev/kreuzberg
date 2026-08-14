---
id: fixture_ruby_extract_batch_uri_all_missing
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```ruby title="Ruby"
require "xberg"
result = Xberg.extract_batch([{ 'kind' => 'uri', 'uri' => '/nonexistent/a.pdf' }, { 'kind' => 'uri', 'uri' => '/nonexistent/b.txt' }])
puts result.inspect

```
