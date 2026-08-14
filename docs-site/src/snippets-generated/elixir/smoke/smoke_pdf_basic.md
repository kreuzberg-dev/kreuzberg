---
id: fixture_elixir_smoke_pdf_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "application/pdf", uri: "https://example.com/pdf/fake_memo.pdf"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
