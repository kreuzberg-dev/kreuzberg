---
id: fixture_wasm_smoke_txt_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.mimeType = "text/plain"; _u0.uri = "https://example.com/text/report.txt"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result);
}

void main();

```
