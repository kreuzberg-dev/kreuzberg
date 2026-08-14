---
id: fixture_ruby_error_extract_input_conflicting_ocr
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```ruby title="Ruby"
require "xberg"
begin
  result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/text/fake_text.txt').bytes, config: { 'disable_ocr' => true, 'force_ocr' => true }, filename: 'fake_text.txt', kind: 'bytes', mime_type: 'text/plain'), { 'disable_ocr' => true, 'force_ocr' => true })
rescue StandardError => error
  warn "#{error.class}: #{error.message}"
end

```
