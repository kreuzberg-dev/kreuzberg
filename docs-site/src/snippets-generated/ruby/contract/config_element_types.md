---
id: fixture_ruby_config_element_types
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/docx/unit_test_headers.docx'), { 'result_format' => 'element_based' })
puts result.inspect

```
