---
id: fixture_wasm_error_empty_bytes
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.bytes = Uint8Array.from([]); _u0.config = (() => { const _u1 = WasmFileExtractionConfig.default(); return _u1; })(); _u0.filename = "empty.txt"; _u0.kind = WasmExtractInputKind.Bytes; _u0.mimeType = "text/plain"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result);
}

void main();

```
