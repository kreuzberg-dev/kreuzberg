---
id: fixture_elixir_extract_bytes_input_invalid_mime
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with unsupported MIME type

```elixir title="Elixir"
try do
  input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/text/plain.txt"), config: %{}, filename: "plain.txt", kind: "bytes", mime_type: "application/x-nonexistent"}
  result = Xberg.extract_async(input_value, "{}")
rescue
  error -> IO.puts(:stderr, "#{inspect(error.__struct__)}: #{Exception.message(error)}")
end

```
