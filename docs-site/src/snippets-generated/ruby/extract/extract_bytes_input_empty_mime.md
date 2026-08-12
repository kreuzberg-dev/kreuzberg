---
id: fixture_ruby_extract_bytes_input_empty_mime
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with empty MIME type

```ruby title="Ruby"
require "xberg"
begin
  result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/text/plain.txt').bytes, config: {  }, filename: 'plain.txt', kind: 'bytes', mime_type: ''), {  })
rescue StandardError => error
  warn "Call failed as expected: #{error.message}"
else
  raise "expected call to fail"
end

```
