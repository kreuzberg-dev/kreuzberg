---
id: fixture_ruby_config_security_limits
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/archives/documents.zip'), { 'security_limits' => { 'max_archive_size' => 104857600, 'max_compression_ratio' => 50, 'max_files_in_archive' => 100 } })
puts result.inspect

```
