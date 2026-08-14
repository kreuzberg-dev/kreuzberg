---
id: fixture_wasm_smoke_json_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "application/json"; _u0.uri = "https://example.com/json/simple.json"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result);
}

void main();

```
