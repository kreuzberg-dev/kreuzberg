---
id: fixture_wasm_api_extract_bytes_input
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = await (async () => { const _u0 = WasmExtractInput.default(); _u0.bytes = await (await import("node:fs/promises")).readFile("test_documents/pdf/fake_memo.pdf"); _u0.filename = "fake_memo.pdf"; _u0.kind = ExtractInputKind.Bytes; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
