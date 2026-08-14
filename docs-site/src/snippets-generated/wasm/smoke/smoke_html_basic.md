---
id: fixture_wasm_smoke_html_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.mimeType = "text/html"; _u0.uri = "https://example.com/html/simple_table.html"; return _u0; })();
  const result = await extract(input, {  });
  const [first] = result.results ?? [];
  for (const table of first?.tables) {
    console.log(table.rows);
  }
}

void main();

```
