---
id: fixture_ruby_format_pptx
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.presentationml.presentation', uri: 'https://example.com/pptx/simple.pptx'))
puts result.inspect

```
