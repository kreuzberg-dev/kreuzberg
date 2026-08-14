---
id: fixture_wasm_extract_batch_empty_inputs
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([], undefined);
  console.log(result);
}

void main();

```
