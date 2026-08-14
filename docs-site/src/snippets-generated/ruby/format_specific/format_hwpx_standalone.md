---
id: fixture_ruby_format_hwpx_standalone
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(filename: 'simple.hwpx', kind: 'uri', mime_type: 'application/haansofthwpx', uri: 'https://example.com/hwpx/simple.hwpx'))
puts result.inspect

```
