---
id: fixture_wasm_config_pages
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, { pages: { extractPages: true, insertPageMarkers: true } });
  console.log(result);
}

void main();

```
