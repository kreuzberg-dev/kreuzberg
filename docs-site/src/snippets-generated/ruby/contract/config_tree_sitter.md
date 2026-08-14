---
id: fixture_ruby_config_tree_sitter
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/code/hello.py'), { 'tree_sitter' => { 'groups' => ['web'], 'languages' => ['python', 'rust'], 'process' => { 'comments' => false, 'diagnostics' => false, 'docstrings' => false, 'exports' => true, 'imports' => true, 'structure' => true, 'symbols' => false } } })
puts result.inspect

```
