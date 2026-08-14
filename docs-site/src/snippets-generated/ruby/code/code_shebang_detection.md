---
id: fixture_ruby_code_shebang_detection
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'text/x-source-code', uri: 'https://example.com/code/script.sh'))
puts result.inspect

```
