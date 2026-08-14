---
id: fixture_wasm_error_extract_input_conflicting_ocr
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, XbergError, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/text/fake_text.txt"); _u0.config = await (async () => { const _u1 = WasmFileExtractionConfig.default(); _u1.disableOcr = true; _u1.forceOcr = true; return _u1; })(); _u0.filename = "fake_text.txt"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = "text/plain"; return _u0; })();
  try {
    await extract(input, { disableOcr: true, forceOcr: true });
  } catch (error) {
    if (error instanceof XbergError) {
      console.error(`${error.name}: ${error.message}`);
    }
  }
}

void main();

```
