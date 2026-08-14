---
id: fixture_ruby_config_llm_structured_extraction
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'structured_extraction' => { 'llm' => { 'model' => 'openai/gpt-4o' }, 'schema' => { 'properties' => { 'date' => { 'type' => 'string' }, 'summary' => { 'type' => 'string' }, 'title' => { 'type' => 'string' } }, 'required' => ['title'], 'type' => 'object' }, 'schema_name' => 'memo_data' } })
puts result.inspect

```
