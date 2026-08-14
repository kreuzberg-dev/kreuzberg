---
id: fixture_wasm_smoke_image_png
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/images/sample.png"; return _u0; })();
  const result = await extract(input, { disableOcr: true });
  console.log(result);
}

void main();

```
