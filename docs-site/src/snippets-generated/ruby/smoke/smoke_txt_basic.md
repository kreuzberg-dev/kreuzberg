---
id: fixture_ruby_smoke_txt_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'text/plain', uri: 'https://example.com/text/report.txt'), {  })
puts result.inspect

```
