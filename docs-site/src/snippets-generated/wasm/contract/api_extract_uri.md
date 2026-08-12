---
id: fixture_wasm_api_extract_uri
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, undefined);
  console.log(result.results[0].content);
}

void main();

```
