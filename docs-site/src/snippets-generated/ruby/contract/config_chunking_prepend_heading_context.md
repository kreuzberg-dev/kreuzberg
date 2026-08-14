---
id: fixture_ruby_config_chunking_prepend_heading_context
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'document.md'), { 'chunking' => { 'chunker_type' => 'markdown', 'max_characters' => 500, 'overlap' => 50, 'prepend_heading_context' => true } })
puts result.inspect

```
