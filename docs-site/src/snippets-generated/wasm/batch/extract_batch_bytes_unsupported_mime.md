---
id: fixture_wasm_extract_batch_bytes_unsupported_mime
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ bytes: [100, 97, 116, 97], kind: "bytes", mimeType: "application/x-unknown" }], undefined);
  console.log(result);
}

void main();

```
