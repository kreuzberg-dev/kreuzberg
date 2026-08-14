---
id: fixture_ruby_url_crawl_linked_pages
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com'), { 'url' => { 'crawl' => { 'max_depth' => 1, 'max_pages' => 4, 'respect_robots_txt' => false }, 'mode' => 'crawl' } })
puts result.inspect

```
