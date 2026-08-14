---
id: fixture_elixir_smoke_image_png
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{kind: "uri", uri: "https://example.com/images/sample.png"}
result = Xberg.extract_async(input_value, "{\"disable_ocr\":true}")
IO.inspect(result)

```
