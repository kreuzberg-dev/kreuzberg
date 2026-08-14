---
id: fixture_ruby_ocr_image_png
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/images/test_hello_world.png').bytes, config: {  }, filename: 'test_hello_world.png', kind: 'bytes', mime_type: 'image/png'), {  })
puts result.inspect

```
