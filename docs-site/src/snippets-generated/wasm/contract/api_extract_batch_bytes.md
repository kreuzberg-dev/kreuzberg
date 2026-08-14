---
id: fixture_wasm_api_extract_batch_bytes
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction API (extract_batch)

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ bytes: "test_documents/pdf/fake_memo.pdf", filename: "fake_memo.pdf", kind: "bytes" }], undefined);
  console.log(result);
}

void main();

```
