---
id: fixture_elixir_smoke_xlsx_basic
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", uri: "https://example.com/xlsx/stanley_cups.xlsx"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
