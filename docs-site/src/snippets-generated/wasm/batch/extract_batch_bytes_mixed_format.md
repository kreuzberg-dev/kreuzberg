---
id: fixture_wasm_extract_batch_bytes_mixed_format
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ bytes: [80, 68, 70, 32, 112, 108, 97, 99, 101, 104, 111, 108, 100, 101, 114], kind: "bytes", mimeType: "application/x-unknown" }], undefined);
  console.log(result);
}

void main();

```
