---
id: fixture_wasm_smoke_xlsx_basic
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.mimeType = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"; _u0.uri = "https://example.com/xlsx/stanley_cups.xlsx"; return _u0; })();
  const result = await extract(input, {  });
  console.log(result);
}

void main();

```
