---
id: fixture_wasm_format_pptx
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "application/vnd.openxmlformats-officedocument.presentationml.presentation"; _u0.uri = "https://example.com/pptx/simple.pptx"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
