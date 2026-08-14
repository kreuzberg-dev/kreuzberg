---
id: fixture_ruby_smoke_html_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'text/html', uri: 'https://example.com/html/simple_table.html'), {  })
puts result.inspect

```
