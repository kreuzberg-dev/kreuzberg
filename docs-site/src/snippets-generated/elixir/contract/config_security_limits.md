---
id: fixture_elixir_config_security_limits
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/archives/documents.zip"}
result = Xberg.extract_async(input_value, "{\"security_limits\":{\"max_archive_size\":104857600,\"max_compression_ratio\":50,\"max_files_in_archive\":100}}")
IO.inspect(result)

```
