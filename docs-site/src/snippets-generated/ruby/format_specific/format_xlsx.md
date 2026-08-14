---
id: fixture_ruby_format_xlsx
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', uri: 'https://example.com/xlsx/stanley_cups.xlsx'))
puts result.inspect

```
