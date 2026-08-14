---
id: fixture_elixir_output_format_bytes_markdown
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/pdf/fake_memo.pdf"), config: %{"output_format" => "markdown"}, filename: "fake_memo.pdf", kind: "bytes", mime_type: "application/pdf"}
result = Xberg.extract_async(input_value, "{\"output_format\":\"markdown\"}")
IO.inspect(result)

```
