---
id: fixture_elixir_extract_bytes_input
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/pdf/fake_memo.pdf"), filename: "fake_memo.pdf", kind: "bytes", mime_type: "application/pdf"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
