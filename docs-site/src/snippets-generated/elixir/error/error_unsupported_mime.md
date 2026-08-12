---
id: fixture_elixir_error_unsupported_mime
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with unsupported MIME type

```elixir title="Elixir"
try do
  input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/text/plain.txt"), config: %{}, filename: "plain.txt", kind: "bytes", mime_type: "application/x-nonexistent"}
  result = Xberg.extract_async(input_value, "{}")
rescue
  error -> IO.puts(:stderr, "Call failed as expected: #{Exception.message(error)}")
else
  _ -> raise "expected call to fail"
end

```
