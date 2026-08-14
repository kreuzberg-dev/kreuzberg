---
id: fixture_ruby_error_empty_mime
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```ruby title="Ruby"
require "xberg"
begin
  result = Xberg.extract(ExtractInput.new(bytes: File.binread('test_documents/text/plain.txt').bytes, config: {  }, filename: 'plain.txt', kind: 'bytes', mime_type: ''), {  })
rescue StandardError => error
  warn "#{error.class}: #{error.message}"
end

```
