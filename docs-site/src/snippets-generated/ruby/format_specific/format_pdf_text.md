---
id: fixture_ruby_format_pdf_text
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(filename: 'fake_memo.pdf', kind: 'uri', mime_type: 'application/pdf', uri: 'https://example.com/pdf/fake_memo.pdf'))
puts result.inspect

```
