---
id: fixture_ruby_smoke_pdf_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/pdf', uri: 'https://example.com/pdf/fake_memo.pdf'), {  })
puts result.inspect

```
