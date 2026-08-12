---
id: fixture_wasm_ocr_image_png
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/images/test_hello_world.png"); _u0.config = await (async () => { const _u1 = WasmFileExtractionConfig.default(); return _u1; })(); _u0.filename = "test_hello_world.png"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = "image/png"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result.results[0].content);
}

void main();

```
