---
id: fixture_ruby_format_docx_equations
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(filename: 'equations.docx', kind: 'uri', mime_type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', uri: 'https://example.com/docx/equations.docx'), { 'output_format' => 'markdown' })

```
