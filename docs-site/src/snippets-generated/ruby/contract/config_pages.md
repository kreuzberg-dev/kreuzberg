---
id: fixture_ruby_config_pages
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'pages' => { 'extract_pages' => true, 'insert_page_markers' => true } })
puts result.inspect

```
