---
id: fixture_ruby_url_recursive_document_urls
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com'), { 'url' => { 'crawl' => { 'document_url_depth' => 1, 'follow_document_urls' => true, 'respect_robots_txt' => false }, 'mode' => 'document' } })
puts result.inspect

```
