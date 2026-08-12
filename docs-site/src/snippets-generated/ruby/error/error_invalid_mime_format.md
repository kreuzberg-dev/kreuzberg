---
id: fixture_ruby_error_invalid_mime_format
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```ruby title="Ruby"
require "xberg"
begin
  result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/text/plain.txt').bytes, config: {  }, filename: 'plain.txt', kind: 'bytes', mime_type: 'not-a-mime'), {  })
rescue StandardError => error
  warn "Call failed as expected: #{error.message}"
else
  raise "expected call to fail"
end

```
