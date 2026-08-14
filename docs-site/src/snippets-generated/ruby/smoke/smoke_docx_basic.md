---
id: fixture_ruby_smoke_docx_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', uri: 'https://example.com/docx/fake.docx'), {  })
puts result.inspect

```
