---
id: fixture_wasm_config_keywords
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, { keywords: { algorithm: "yake", maxKeywords: 10 } });
  const [first] = result.results ?? [];
  for (const keyword of first?.keywords) {
    console.log(keyword.text);
    console.log(keyword.score);
  }
}

void main();

```
