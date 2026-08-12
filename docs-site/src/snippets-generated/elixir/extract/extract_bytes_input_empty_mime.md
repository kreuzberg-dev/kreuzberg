---
id: fixture_elixir_extract_bytes_input_empty_mime
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with empty MIME type

```elixir title="Elixir"
try do
  input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/text/plain.txt"), config: %{}, filename: "plain.txt", kind: "bytes", mime_type: ""}
  result = Xberg.extract_async(input_value, "{}")
rescue
  error -> IO.puts(:stderr, "Call failed as expected: #{Exception.message(error)}")
else
  _ -> raise "expected call to fail"
end

```
