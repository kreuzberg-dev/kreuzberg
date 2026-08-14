---
id: fixture_ruby_url_html_page_extract
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com'), { 'url' => { 'mode' => 'document' } })
puts result.inspect

```
