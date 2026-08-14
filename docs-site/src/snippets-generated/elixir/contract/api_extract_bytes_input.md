---
id: fixture_elixir_api_extract_bytes_input
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/pdf/fake_memo.pdf"), filename: "fake_memo.pdf", kind: "bytes"}
result = Xberg.extract_async(input_value)
IO.inspect(result)

```
