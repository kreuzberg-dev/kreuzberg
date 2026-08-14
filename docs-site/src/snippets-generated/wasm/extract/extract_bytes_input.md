---
id: fixture_wasm_extract_bytes_input
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"); _u0.filename = "fake_memo.pdf"; _u0.kind = ExtractInputKind.Bytes; _u0.mimeType = "application/pdf"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
