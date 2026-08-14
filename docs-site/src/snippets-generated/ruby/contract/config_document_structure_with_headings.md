---
id: fixture_ruby_config_document_structure_with_headings
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/docx/fake.docx'), { 'include_document_structure' => true })
puts result.inspect

```
