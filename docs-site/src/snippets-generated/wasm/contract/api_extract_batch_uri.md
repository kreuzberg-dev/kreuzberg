---
id: fixture_wasm_api_extract_batch_uri
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf" }], undefined);
  console.log(result);
}

void main();

```
