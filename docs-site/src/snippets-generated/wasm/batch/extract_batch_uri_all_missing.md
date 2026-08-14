---
id: fixture_wasm_extract_batch_uri_all_missing
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```typescript title="WebAssembly"
import { extractBatch } from "@xberg-io/xberg-wasm";
async function main() {
  const result = await extractBatch([{ kind: "uri", uri: "/nonexistent/a.pdf" }, { kind: "uri", uri: "/nonexistent/b.txt" }], undefined);
  console.log(result);
}

void main();

```
