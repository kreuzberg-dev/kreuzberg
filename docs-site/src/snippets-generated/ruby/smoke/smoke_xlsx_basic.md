---
id: fixture_ruby_smoke_xlsx_basic
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', uri: 'https://example.com/xlsx/stanley_cups.xlsx'), {  })
puts result.inspect

```
