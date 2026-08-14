---
id: fixture_elixir_error_extract_input_conflicting_ocr
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```elixir title="Elixir"
try do
  input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/text/fake_text.txt"), config: %{"disable_ocr" => true, "force_ocr" => true}, filename: "fake_text.txt", kind: "bytes", mime_type: "text/plain"}
  result = Xberg.extract_async(input_value, "{\"disable_ocr\":true,\"force_ocr\":true}")
rescue
  error -> IO.puts(:stderr, "#{inspect(error.__struct__)}: #{Exception.message(error)}")
end

```
