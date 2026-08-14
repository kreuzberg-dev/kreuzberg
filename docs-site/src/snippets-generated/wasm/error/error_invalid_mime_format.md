---
id: fixture_wasm_error_invalid_mime_format
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, XbergError, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/text/plain.txt"); _u0.config = await (async () => { const _u1 = WasmFileExtractionConfig.default(); return _u1; })(); _u0.filename = "plain.txt"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = "not-a-mime"; return _u0; })();
  try {
    await extract(input, {  });
  } catch (error) {
    if (error instanceof XbergError) {
      console.error(`${error.name}: ${error.message}`);
    }
  }
}

void main();

```
