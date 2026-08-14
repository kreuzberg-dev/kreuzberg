---
id: fixture_wasm_format_pdf_text
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.filename = "fake_memo.pdf"; _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "application/pdf"; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result.results[0].metadata);
}

void main();

```
