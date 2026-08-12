---
id: fixture_elixir_extract_batch_bytes_size_cap
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```elixir title="Elixir"
try do
  result = Xberg.extract_batch_async([%{"bytes" => "test_documents/text/fake_text.txt", "kind" => "bytes", "mime_type" => "text/plain"}], "{\"security_limits\":{\"max_content_size\":1}}")
rescue
  error -> IO.puts(:stderr, "Call failed as expected: #{Exception.message(error)}")
else
  _ -> raise "expected call to fail"
end

```
