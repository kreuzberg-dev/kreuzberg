---
id: fixture_ruby_ocr_backends_list
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

List all registered OCR backends

```ruby title="Ruby"
require "xberg"
result = Xberg.list_ocr_backends()
puts result.inspect

```
