---
id: fixture_elixir_format_pdf_text
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{filename: "fake_memo.pdf", kind: "uri", mime_type: "application/pdf", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
