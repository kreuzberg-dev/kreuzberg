---
id: fixture_wasm_extract_batch_uri_partial_failure
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "text/plain.txt" }, { kind: "uri", uri: "/nonexistent/missing.pdf" }], undefined);
  console.log(result);
}

void main();

```
