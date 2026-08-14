---
id: fixture_wasm_extract_batch_bytes_invalid_mime
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ bytes: [72, 101, 108, 108, 111], kind: "bytes", mimeType: "application/x-nonexistent" }], undefined);
  console.log(result);
}

void main();

```
