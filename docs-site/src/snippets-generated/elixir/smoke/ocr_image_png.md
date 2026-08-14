---
id: fixture_elixir_ocr_image_png
language: elixir
target: elixir
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```elixir title="Elixir"
input_value = %Xberg.ExtractInput{bytes: File.read!("test_documents/images/test_hello_world.png"), config: %{}, filename: "test_hello_world.png", kind: "bytes", mime_type: "image/png"}
result = Xberg.extract_async(input_value, "{}")
IO.inspect(result)

```
