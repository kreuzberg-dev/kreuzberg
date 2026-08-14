---
id: fixture_wasm_code_shebang_detection
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "text/x-source-code"; _u0.uri = "https://example.com/code/script.sh"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
