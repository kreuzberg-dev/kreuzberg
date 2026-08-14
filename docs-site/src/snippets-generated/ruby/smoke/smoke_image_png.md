---
id: fixture_ruby_smoke_image_png
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/images/sample.png'), { 'disable_ocr' => true })
puts result.inspect

```
