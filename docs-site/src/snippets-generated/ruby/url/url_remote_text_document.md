---
id: fixture_ruby_url_remote_text_document
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com'), { 'url' => { 'mode' => 'document' } })
puts result.inspect

```
