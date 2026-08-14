---
id: fixture_ruby_extract_batch_bytes_size_cap
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```ruby title="Ruby"
require "xberg"
begin
  result = Xberg.extract_batch([{ 'bytes' => 'test_documents/text/fake_text.txt', 'kind' => 'bytes', 'mime_type' => 'text/plain' }], { 'security_limits' => { 'max_content_size' => 1 } })
rescue StandardError => error
  warn "#{error.class}: #{error.message}"
end

```
