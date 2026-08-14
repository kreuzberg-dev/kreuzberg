---
id: fixture_ruby_format_docx_standalone
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(filename: 'fake.docx', kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', uri: 'https://example.com/docx/fake.docx'))
puts result.inspect

```
