---
id: fixture_wasm_extract_batch_uri_not_found
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "/nonexistent/a.pdf" }], undefined);
  console.log(result);
}

void main();

```
