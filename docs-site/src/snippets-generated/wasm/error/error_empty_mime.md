---
id: fixture_wasm_error_empty_mime
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, XbergError, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/text/plain.txt"); _u0.config = await (async () => { const _u1 = WasmFileExtractionConfig.default(); return _u1; })(); _u0.filename = "plain.txt"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = ""; return _u0; })();
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
