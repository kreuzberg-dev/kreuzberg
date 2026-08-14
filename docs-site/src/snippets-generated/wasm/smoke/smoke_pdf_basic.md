---
id: fixture_wasm_smoke_pdf_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.mimeType = "application/pdf"; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result);
}

void main();

```
